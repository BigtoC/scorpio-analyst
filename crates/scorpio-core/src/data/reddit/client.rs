//! HTTP wrapper around Reddit's anonymous search endpoints.
//!
//! Primary path: `search.json`. On HTTP 403 the client transparently retries
//! via the public Atom/RSS feed (`search.rss`). RSS-sourced submissions are
//! marked `via_rss: true` so the provider can omit unavailable metrics (score,
//! comment count) rather than displaying fake zeros.
//!
//! Rate-limits all outbound requests via [`SharedRateLimiter`] and maps
//! transport/timeout/malformed-JSON failures to [`TradingError`].

use std::time::Duration;

use reqwest::header::USER_AGENT;
use tracing::warn;

use crate::{
    constants::REDDIT_REQUEST_TIMEOUT_SECS, error::TradingError, rate_limit::SharedRateLimiter,
};

use super::types::{RawListing, RawSubmission};

/// Default Reddit base URL. Overridable in tests via [`RedditClient::with_base_url`].
const DEFAULT_BASE_URL: &str = "https://www.reddit.com";

/// HTTP client for Reddit's anonymous JSON endpoints.
#[derive(Clone, Debug)]
pub struct RedditClient {
    http: reqwest::Client,
    limiter: SharedRateLimiter,
    user_agent: String,
    base_url: String,
}

impl RedditClient {
    /// Construct a production client.
    ///
    /// The full UA header is built from
    /// `format!("{REDDIT_USER_AGENT_PREFIX}/{} (https://github.com/BigtoC/scorpio-analyst)", env!("CARGO_PKG_VERSION"))`
    /// at caller construction time and passed in here so it can be unit-tested
    /// without env-var coupling.
    #[must_use]
    pub fn new(http: reqwest::Client, limiter: SharedRateLimiter, user_agent: String) -> Self {
        Self {
            http,
            limiter,
            user_agent,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Construct a non-functional client for use in tests only.
    ///
    /// Uses a no-op limiter and the default base URL; tests that need to
    /// hit a mock server should chain [`Self::with_base_url`].
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(REDDIT_REQUEST_TIMEOUT_SECS))
                .build()
                .expect("test client build"),
            limiter: SharedRateLimiter::disabled("test-reddit"),
            user_agent: "scorpio-analyst-test/0.0.0".to_owned(),
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Override the base URL for HTTP stubbing.
    #[doc(hidden)]
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Build the `search.json` URL for a multi-subreddit query.
    pub(crate) fn build_search_url(
        &self,
        subreddits: &[String],
        query: &str,
        limit: u32,
    ) -> String {
        self.build_url(subreddits, query, limit, "json")
    }

    /// Build the `search.rss` URL — same query string, `.rss` suffix.
    pub(crate) fn build_rss_url(&self, subreddits: &[String], query: &str, limit: u32) -> String {
        self.build_url(subreddits, query, limit, "rss")
    }

    fn build_url(&self, subreddits: &[String], query: &str, limit: u32, fmt: &str) -> String {
        let joined = subreddits.join("+");
        let encoded_q = url_encode(query);
        format!(
            "{base}/r/{subs}/search.{fmt}?q={q}&restrict_sr=on&sort=new&over_18=false&stickied=false&limit={limit}",
            base = self.base_url.trim_end_matches('/'),
            subs = joined,
            q = encoded_q,
        )
    }

    /// Search submissions across the configured subreddits.
    ///
    /// Acquires a rate-limit permit before issuing the request. See module
    /// docs for the full error-mapping contract.
    pub async fn search_submissions(
        &self,
        subreddits: &[String],
        query: &str,
        limit: u32,
    ) -> Result<Vec<RawSubmission>, TradingError> {
        self.limiter.acquire().await;

        let url = self.build_search_url(subreddits, query, limit);
        let request = self.http.get(&url).header(USER_AGENT, &self.user_agent);

        let response = request.send().await.map_err(map_transport_err)?;

        let status = response.status();
        if status.as_u16() == 429 {
            return Err(TradingError::NetworkTimeout {
                elapsed: Duration::ZERO,
                message: format!("reddit: rate-limited (HTTP {status})"),
            });
        }
        if status.is_server_error() {
            return Err(TradingError::NetworkTimeout {
                elapsed: Duration::ZERO,
                message: format!("reddit: upstream error (HTTP {status})"),
            });
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            // JSON API blocked — transparently retry via public Atom/RSS feed.
            tracing::debug!(symbol = query, "reddit JSON 403; falling back to RSS feed");
            return Ok(self.fetch_rss(subreddits, query, limit).await);
        }
        if !status.is_success() {
            return Err(TradingError::AnalystError {
                agent: "reddit".to_owned(),
                message: format!("reddit: unexpected HTTP status {status}"),
            });
        }

        let bytes = response.bytes().await.map_err(map_transport_err)?;

        let listing: RawListing = serde_json::from_slice(&bytes).map_err(|err| {
            warn!(
                error = %err,
                error.kind = "deserialize",
                "reddit response parse failed"
            );
            TradingError::SchemaViolation {
                message: "reddit: response body could not be parsed as a listing".to_owned(),
            }
        })?;

        Ok(listing.data.children.into_iter().map(|c| c.data).collect())
    }
}

impl RedditClient {
    /// Fetch posts via the public Atom/RSS feed. Returns an empty vec on any
    /// failure so the caller always gets a usable (possibly empty) result.
    async fn fetch_rss(
        &self,
        subreddits: &[String],
        query: &str,
        limit: u32,
    ) -> Vec<RawSubmission> {
        let url = self.build_rss_url(subreddits, query, limit);
        let response = match self
            .http
            .get(&url)
            .header(USER_AGENT, &self.user_agent)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "reddit RSS request failed");
                return vec![];
            }
        };
        if !response.status().is_success() {
            tracing::debug!(status = %response.status(), "reddit RSS returned non-200");
            return vec![];
        }
        match response.bytes().await {
            Ok(bytes) => parse_atom_feed(&bytes),
            Err(e) => {
                tracing::debug!(error = %e, "reddit RSS body read failed");
                vec![]
            }
        }
    }
}

/// Parse a Reddit Atom/RSS feed, returning one `RawSubmission` per `<entry>`.
///
/// Fields absent from the Atom spec (score, over_18, stickied) are set to
/// their safe defaults. `via_rss` is always `true` on returned submissions.
pub(crate) fn parse_atom_feed(bytes: &[u8]) -> Vec<RawSubmission> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let Ok(text) = std::str::from_utf8(bytes) else {
        return vec![];
    };

    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut in_entry = false;
    // Per-entry byte accumulators (like EDGAR pattern — reassembled across
    // Text + GeneralRef + CData events).
    let mut title_buf: Vec<u8> = Vec::new();
    let mut content_buf: Vec<u8> = Vec::new();
    let mut published_buf: Vec<u8> = Vec::new();
    let mut permalink = String::new();
    let mut subreddit = String::new();

    // Which accumulator is active.
    let mut in_title = false;
    let mut in_content = false;
    let mut in_published = false;

    loop {
        match reader.read_event() {
            Err(_) | Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name_vec = e.name().as_ref().to_vec();
                let tag = tag_name(&name_vec);
                match tag {
                    "entry" => {
                        in_entry = true;
                        title_buf.clear();
                        content_buf.clear();
                        published_buf.clear();
                        permalink.clear();
                        subreddit.clear();
                        in_title = false;
                        in_content = false;
                        in_published = false;
                    }
                    "title" if in_entry => {
                        in_title = true;
                        title_buf.clear();
                    }
                    "content" if in_entry => {
                        in_content = true;
                        content_buf.clear();
                    }
                    "published" if in_entry => {
                        in_published = true;
                        published_buf.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if !in_entry {
                    continue;
                }
                let name_vec = e.name().as_ref().to_vec();
                let tag = tag_name(&name_vec);
                match tag {
                    "link" => {
                        let mut is_alternate = false;
                        let mut href = String::new();
                        for attr in e.attributes().flatten() {
                            let k = attr.key.as_ref().to_vec();
                            let v = String::from_utf8_lossy(&attr.value).into_owned();
                            match k.as_slice() {
                                b"rel" => is_alternate = v == "alternate",
                                b"href" => href = v,
                                _ => {}
                            }
                        }
                        if is_alternate && !href.is_empty() {
                            if subreddit.is_empty()
                                && let Some(s) = extract_subreddit_from_url(&href)
                            {
                                subreddit = s;
                            }
                            permalink = href;
                        }
                    }
                    "category" if subreddit.is_empty() => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref().to_vec().as_slice() == b"term" {
                                subreddit = String::from_utf8_lossy(&attr.value).into_owned();
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if !in_entry {
                    continue;
                }
                let decoded = match t.decode() {
                    Ok(s) => s.into_owned(),
                    Err(_) => String::from_utf8_lossy(t.into_inner().as_ref()).into_owned(),
                };
                if in_title {
                    title_buf.extend_from_slice(decoded.as_bytes());
                } else if in_content {
                    content_buf.extend_from_slice(decoded.as_bytes());
                } else if in_published {
                    published_buf.extend_from_slice(decoded.as_bytes());
                }
            }
            Ok(Event::CData(c)) => {
                if !in_entry {
                    continue;
                }
                let raw = c.into_inner();
                if in_title {
                    title_buf.extend_from_slice(raw.as_ref());
                } else if in_content {
                    content_buf.extend_from_slice(raw.as_ref());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if !in_entry {
                    continue;
                }
                let resolved = resolve_entity_ref(&r);
                let s = resolved.map(|c| c.to_string()).unwrap_or_default();
                if in_title {
                    title_buf.extend_from_slice(s.as_bytes());
                } else if in_content {
                    content_buf.extend_from_slice(s.as_bytes());
                }
            }
            Ok(Event::End(e)) => {
                let name_vec = e.name().as_ref().to_vec();
                let tag = tag_name(&name_vec);
                match tag {
                    "entry" if in_entry => {
                        in_entry = false;
                        in_title = false;
                        in_content = false;
                        in_published = false;

                        let title = String::from_utf8_lossy(&title_buf).trim().to_owned();
                        let content_html = String::from_utf8_lossy(&content_buf).into_owned();
                        let selftext = strip_html(content_html.trim());
                        let published_str = String::from_utf8_lossy(&published_buf).into_owned();
                        let created_utc = parse_rfc3339_to_timestamp(published_str.trim());
                        let relative_permalink = to_relative_permalink(&permalink);

                        out.push(RawSubmission {
                            title,
                            selftext,
                            permalink: relative_permalink,
                            subreddit: subreddit.clone(),
                            created_utc,
                            score: 0,
                            over_18: false,
                            stickied: false,
                            via_rss: true,
                        });
                    }
                    "title" => in_title = false,
                    "content" => in_content = false,
                    "published" => in_published = false,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    out
}

/// Strip the namespace prefix from a tag name, e.g. `"atom:entry"` → `"entry"`.
fn tag_name(raw: &[u8]) -> &str {
    let s = std::str::from_utf8(raw).unwrap_or("");
    s.rsplit_once(':').map(|(_, local)| local).unwrap_or(s)
}

/// Extract subreddit name from a Reddit permalink URL.
///
/// e.g. `https://www.reddit.com/r/stocks/comments/abc/xyz/` → `"stocks"`
fn extract_subreddit_from_url(url: &str) -> Option<String> {
    let after_r = url.split("/r/").nth(1)?;
    let sub = after_r.split('/').next()?;
    if sub.is_empty() {
        None
    } else {
        Some(sub.to_owned())
    }
}

/// Convert an absolute Reddit URL to a relative permalink matching JSON format.
///
/// e.g. `https://www.reddit.com/r/stocks/comments/abc/xyz/` → `/r/stocks/comments/abc/xyz/`
fn to_relative_permalink(url: &str) -> String {
    url.strip_prefix("https://www.reddit.com")
        .or_else(|| url.strip_prefix("http://www.reddit.com"))
        .map(str::to_owned)
        .unwrap_or_else(|| url.to_owned())
}

/// Parse an RFC 3339 timestamp string to a Unix epoch float.
fn parse_rfc3339_to_timestamp(s: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp() as f64)
        .unwrap_or(0.0)
}

/// Strip HTML tags and Reddit SC markers from Atom `<content>` text.
fn strip_html(input: &str) -> String {
    let s = input
        .replace("<!-- SC_OFF -->", "")
        .replace("<!-- SC_ON -->", "");
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// Resolve an XML named entity reference to a character.
///
/// Uses `resolve_char_ref` for numeric references (`&#38;`, `&#x26;`) then
/// falls back to the five predefined XML entities.
fn resolve_entity_ref(r: &quick_xml::events::BytesRef<'_>) -> Option<char> {
    if let Ok(Some(ch)) = r.resolve_char_ref() {
        return Some(ch);
    }
    match r.decode().ok()?.as_ref() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        _ => None,
    }
}

fn map_transport_err(err: reqwest::Error) -> TradingError {
    if err.is_timeout() {
        return TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(REDDIT_REQUEST_TIMEOUT_SECS),
            message: format!("reddit: request timed out: {err}"),
        };
    }
    TradingError::NetworkTimeout {
        elapsed: Duration::ZERO,
        message: format!("reddit: transport error: {err}"),
    }
}

/// Minimal percent-encoder for the `q` query parameter.
///
/// Only the characters that matter for tickers / multi-word queries are
/// encoded. Reddit's URL parser is lenient; this is enough for `AAPL`,
/// `BRK.B`, and the multi-token denylist look-alikes we send.
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> RedditClient {
        RedditClient::for_test()
    }

    #[test]
    fn build_search_url_joins_subreddits_with_plus() {
        let c = test_client();
        let url = c.build_search_url(&["stocks".to_owned(), "investing".to_owned()], "AAPL", 100);
        assert!(url.contains("/r/stocks+investing/search.json"), "url={url}");
    }

    #[test]
    fn build_search_url_includes_all_required_query_params() {
        let c = test_client();
        let url = c.build_search_url(&["stocks".to_owned()], "AAPL", 100);
        for token in [
            "q=AAPL",
            "restrict_sr=on",
            "sort=new",
            "over_18=false",
            "stickied=false",
            "limit=100",
        ] {
            assert!(
                url.contains(token),
                "url must contain '{token}', got: {url}"
            );
        }
    }

    #[test]
    fn build_search_url_percent_encodes_unusual_chars() {
        let c = test_client();
        let url = c.build_search_url(&["stocks".to_owned()], "BRK.B", 50);
        // "." is unreserved per RFC3986 — must NOT be encoded.
        assert!(url.contains("q=BRK.B"), "got: {url}");

        let url = c.build_search_url(&["stocks".to_owned()], "A B", 50);
        assert!(url.contains("q=A%20B"), "space must be encoded; got: {url}");
    }

    #[test]
    fn url_encode_unreserved_passthrough() {
        assert_eq!(url_encode("AAPL"), "AAPL");
        assert_eq!(url_encode("BRK.B"), "BRK.B");
        assert_eq!(url_encode("A_b-c~d"), "A_b-c~d");
    }

    #[test]
    fn url_encode_special_chars() {
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("a&b"), "a%26b");
    }

    // ── Atom/RSS parser ──────────────────────────────────────────────────

    const ATOM_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <title>AAPL earnings discussion</title>
    <link rel="alternate" href="https://www.reddit.com/r/stocks/comments/abc/aapl_post/"/>
    <published>2024-04-15T12:00:00+00:00</published>
    <content type="html">&lt;!-- SC_OFF --&gt;&lt;div class="md"&gt;&lt;p&gt;Great results this quarter.&lt;/p&gt;&lt;/div&gt;&lt;!-- SC_ON --&gt;</content>
    <category term="stocks" label="r/stocks"/>
  </entry>
  <entry>
    <title>Another post</title>
    <link rel="alternate" href="https://www.reddit.com/r/investing/comments/xyz/post/"/>
    <published>2024-04-14T08:30:00+00:00</published>
    <content type="html">&lt;p&gt;Some content&lt;/p&gt;</content>
    <category term="investing" label="r/investing"/>
  </entry>
</feed>"#;

    #[test]
    fn parse_atom_feed_extracts_entries() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert_eq!(posts.len(), 2);
    }

    #[test]
    fn parse_atom_feed_sets_via_rss_true() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert!(posts.iter().all(|p| p.via_rss));
    }

    #[test]
    fn parse_atom_feed_parses_title_and_subreddit() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert_eq!(posts[0].title, "AAPL earnings discussion");
        assert_eq!(posts[0].subreddit, "stocks");
        assert_eq!(posts[1].subreddit, "investing");
    }

    #[test]
    fn parse_atom_feed_permalink_is_relative() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert_eq!(posts[0].permalink, "/r/stocks/comments/abc/aapl_post/");
    }

    #[test]
    fn parse_atom_feed_strips_html_from_content() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert!(!posts[0].selftext.contains('<'), "should strip HTML tags");
        assert!(
            posts[0].selftext.contains("Great results"),
            "should keep text"
        );
        assert!(
            !posts[0].selftext.contains("SC_OFF"),
            "should strip SC markers"
        );
    }

    #[test]
    fn parse_atom_feed_score_is_zero() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        assert!(posts.iter().all(|p| p.score == 0));
    }

    #[test]
    fn parse_atom_feed_published_to_unix_timestamp() {
        let posts = parse_atom_feed(ATOM_FEED.as_bytes());
        // 2024-04-15T12:00:00+00:00 = 1713182400
        assert!((posts[0].created_utc - 1_713_182_400.0).abs() < 1.0);
    }

    #[test]
    fn build_rss_url_uses_rss_suffix() {
        let c = test_client();
        let url = c.build_rss_url(&["stocks".to_owned()], "AAPL", 10);
        assert!(url.contains("search.rss"), "got: {url}");
        assert!(url.contains("q=AAPL"), "got: {url}");
    }

    // ── HTTP wire tests against wiremock ─────────────────────────────────

    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SAMPLE_RESPONSE: &str = r#"{
        "kind": "Listing",
        "data": {
            "children": [{
                "kind": "t3",
                "data": {
                    "title": "AAPL discussion",
                    "selftext": "body",
                    "permalink": "/r/stocks/comments/abc/aapl/",
                    "subreddit": "stocks",
                    "created_utc": 1713200000.0,
                    "score": 100,
                    "over_18": false,
                    "stickied": false
                }
            }]
        }
    }"#;

    fn http_client_for_test() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .expect("http client")
    }

    fn client_against(server: &MockServer) -> RedditClient {
        let http = http_client_for_test();
        RedditClient::new(
            http,
            SharedRateLimiter::disabled("test-reddit"),
            "scorpio-analyst-test/0.0.0".to_owned(),
        )
        .with_base_url(server.uri())
    }

    #[tokio::test]
    async fn sends_user_agent_and_parses_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/r/stocks/search.json"))
            .and(query_param("q", "AAPL"))
            .and(query_param("restrict_sr", "on"))
            .and(query_param("sort", "new"))
            .and(query_param("limit", "100"))
            .and(header("user-agent", "scorpio-analyst-test/0.0.0"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SAMPLE_RESPONSE))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_against(&server);
        let posts = client
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect("ok");
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "AAPL discussion");
    }

    #[tokio::test]
    async fn empty_listing_returns_empty_vec() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{ "kind": "Listing", "data": { "children": [] } }"#),
            )
            .mount(&server)
            .await;

        let posts = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect("ok");
        assert!(posts.is_empty());
    }

    #[tokio::test]
    async fn rate_limited_429_maps_to_network_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let err = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect_err("should fail");
        assert!(matches!(err, TradingError::NetworkTimeout { .. }));
        assert!(format!("{err}").to_lowercase().contains("rate-limited"));
    }

    #[tokio::test]
    async fn server_5xx_maps_to_network_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let err = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect_err("should fail");
        assert!(matches!(err, TradingError::NetworkTimeout { .. }));
    }

    #[tokio::test]
    async fn timeout_maps_to_network_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(SAMPLE_RESPONSE)
                    .set_delay(std::time::Duration::from_millis(2_000)),
            )
            .mount(&server)
            .await;

        let err = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect_err("should time out");
        assert!(matches!(err, TradingError::NetworkTimeout { .. }));
    }

    #[tokio::test]
    async fn forbidden_403_on_json_falls_back_to_rss_and_succeeds() {
        let server = MockServer::start().await;
        // JSON endpoint returns 403.
        Mock::given(method("GET"))
            .and(path("/r/stocks/search.json"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        // RSS endpoint returns a valid Atom feed.
        Mock::given(method("GET"))
            .and(path("/r/stocks/search.rss"))
            .respond_with(ResponseTemplate::new(200).set_body_string(ATOM_FEED))
            .mount(&server)
            .await;

        let posts = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect("403 should transparently fall back to RSS");
        assert!(!posts.is_empty(), "RSS fallback should return posts");
        assert!(
            posts.iter().all(|p| p.via_rss),
            "fallback posts must be marked via_rss"
        );
    }

    #[tokio::test]
    async fn forbidden_403_on_json_and_rss_returns_empty_ok() {
        let server = MockServer::start().await;
        // Both endpoints fail.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let posts = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect("double-403 should return Ok(empty) not Err");
        assert!(posts.is_empty());
    }

    #[tokio::test]
    async fn malformed_json_maps_to_schema_violation() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{ not valid json"))
            .mount(&server)
            .await;

        let err = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect_err("should fail");
        assert!(matches!(err, TradingError::SchemaViolation { .. }));
    }
}
