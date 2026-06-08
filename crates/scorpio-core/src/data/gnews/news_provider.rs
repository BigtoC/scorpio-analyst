//! [`GnewsNewsProvider`] — Google News RSS vetted news feed.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::USER_AGENT;
use rss::Channel;

use crate::{
    constants::{NEWS_ANALYSIS_DAYS, NEWS_SNIPPET_MAX_CHARS, NEWS_TITLE_MAX_CHARS},
    data::traits::NewsProvider,
    domain::Symbol,
    error::TradingError,
    state::{NewsArticle, NewsData},
};

/// Google News base URL. Overridable in tests via [`GnewsNewsProvider::with_base_url`].
const DEFAULT_BASE_URL: &str = "https://news.google.com";

/// Timeout for the single Google News RSS request.
const GNEWS_REQUEST_TIMEOUT_SECS: u64 = 10;

/// User-Agent sent on the RSS request.
const GNEWS_USER_AGENT: &str = concat!("scorpio-analyst/", env!("CARGO_PKG_VERSION"));

/// Google News RSS news provider.
///
/// Queries `news.google.com/rss/search?q=<ticker>` and maps each RSS `<item>`
/// to a [`NewsArticle`]. Requires no API key. Only equity symbols produce a
/// meaningful search; non-equity symbols return empty `NewsData` immediately.
///
/// Unlike the `gnews-rs` crate this replaced, it makes a **single** HTTP
/// request and reads `title`/`link`/`source`/`pubDate`/`description` straight
/// off the feed. It performs no per-article page scraping for origin links or
/// thumbnails (data this pipeline discards), so it avoids the `1 + 2N` request
/// fan-out and the "relative URL without a base" scrape-failure log noise.
#[derive(Clone, Debug)]
pub struct GnewsNewsProvider {
    http: reqwest::Client,
    base_url: String,
}

impl Default for GnewsNewsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GnewsNewsProvider {
    #[must_use]
    pub fn new() -> Self {
        // The only failure path for `build()` is process-fatal TLS init, which
        // would equally doom every other HTTP client in the process; degrade to
        // the default client rather than surfacing an unrecoverable error. See
        // `.claude/rules/infallible-constructor-for-process-fatal-failures.md`.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(GNEWS_REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Override the base URL for HTTP stubbing in tests.
    #[doc(hidden)]
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn ticker_for_search(symbol: &Symbol) -> Option<String> {
        symbol.as_equity().map(|t| t.as_str().to_owned())
    }

    /// Build the Google News RSS search URL for `query`.
    fn build_search_url(&self, query: &str) -> String {
        format!(
            "{base}/rss/search?q={query}",
            base = self.base_url.trim_end_matches('/'),
        )
    }
}

/// Extract the host component from a URL string for display as the article source.
fn extract_host(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .to_owned()
}

/// Strip HTML tags from a string.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// Parse an RFC 2822 `pubDate` string to a UTC datetime.
fn parse_pub_date(pub_date: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(pub_date)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

/// Map an RSS channel's items to [`NewsArticle`]s, dropping items published
/// before `cutoff` or with a missing/unparseable publish date.
fn map_articles(channel: &Channel, cutoff: DateTime<Utc>) -> Vec<NewsArticle> {
    channel
        .items()
        .iter()
        .filter_map(|item| {
            let published = parse_pub_date(item.pub_date()?)?;
            if published < cutoff {
                return None;
            }
            let url = item.link().filter(|l| !l.is_empty()).map(str::to_owned);
            let source = match item.source() {
                Some(s) if !s.url().is_empty() => extract_host(s.url()),
                _ => url
                    .as_deref()
                    .map(extract_host)
                    .unwrap_or_else(|| "Google News".to_owned()),
            };
            Some(NewsArticle {
                title: truncate_chars(item.title().unwrap_or_default(), NEWS_TITLE_MAX_CHARS),
                source,
                published_at: published.to_rfc3339(),
                relevance_score: None,
                snippet: truncate_chars(
                    &strip_html(item.description().unwrap_or_default()),
                    NEWS_SNIPPET_MAX_CHARS,
                ),
                url,
            })
        })
        .collect()
}

/// Map a `reqwest` transport failure to a [`TradingError::NetworkTimeout`].
fn map_transport_err(err: reqwest::Error) -> TradingError {
    if err.is_timeout() {
        return TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(GNEWS_REQUEST_TIMEOUT_SECS),
            message: format!("gnews: request timed out: {err}"),
        };
    }
    TradingError::NetworkTimeout {
        elapsed: Duration::ZERO,
        message: format!("gnews: transport error: {err}"),
    }
}

#[async_trait]
impl NewsProvider for GnewsNewsProvider {
    fn provider_name(&self) -> &'static str {
        "gnews"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        let Some(ticker) = Self::ticker_for_search(symbol) else {
            return Ok(NewsData {
                articles: vec![],
                macro_events: vec![],
                summary: "gnews: 0 articles (unsupported symbol shape)".to_owned(),
            });
        };

        let url = self.build_search_url(&ticker);
        let response = self
            .http
            .get(&url)
            .header(USER_AGENT, GNEWS_USER_AGENT)
            .send()
            .await
            .map_err(map_transport_err)?;

        let status = response.status();
        if !status.is_success() {
            return Err(TradingError::AnalystError {
                agent: "gnews".to_owned(),
                message: format!("gnews: unexpected HTTP status {status}"),
            });
        }

        let bytes = response.bytes().await.map_err(map_transport_err)?;

        // Map the RSS parse error to a `TradingError`. `rss::Error` is
        // `Send + Sync`, so — unlike the old `gnews-rs` path whose error was
        // `Box<dyn Error>` (`!Send`) and forced a `spawn_blocking`/nested-runtime
        // workaround — this needs no such hack; the future stays `Send`.
        let channel =
            Channel::read_from(&bytes[..]).map_err(|err| TradingError::SchemaViolation {
                message: format!("gnews: response body could not be parsed as RSS: {err}"),
            })?;

        let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;
        let articles = map_articles(&channel, cutoff);
        let count = articles.len();
        Ok(NewsData {
            articles,
            macro_events: vec![],
            summary: format!("gnews: {count} articles"),
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(
            extract_host("https://www.bloomberg.com/news/article"),
            "www.bloomberg.com"
        );
        assert_eq!(extract_host("http://reuters.com/"), "reuters.com");
        assert_eq!(extract_host("bloomberg.com"), "bloomberg.com");
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<p>Hello world</p>"), "Hello world");
        assert_eq!(strip_html("<a href=\"x\">click</a>"), "click");
        assert_eq!(strip_html("plain text"), "plain text");
    }

    #[test]
    fn parse_pub_date_rfc2822_to_utc() {
        let input = "Thu, 05 Jun 2025 10:00:00 GMT";
        let dt = parse_pub_date(input).expect("valid RFC 2822");
        assert_eq!(dt.to_rfc3339(), "2025-06-05T10:00:00+00:00");
    }

    #[test]
    fn parse_pub_date_returns_none_for_invalid_input() {
        assert!(parse_pub_date("not a date").is_none());
        assert!(parse_pub_date("").is_none());
    }

    #[test]
    fn build_search_url_appends_query() {
        let provider = GnewsNewsProvider::new().with_base_url("https://news.example.com/");
        let url = provider.build_search_url("AAPL");
        assert_eq!(url, "https://news.example.com/rss/search?q=AAPL");
    }

    // ── Pure RSS → NewsArticle mapping ───────────────────────────────────

    const SAMPLE_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
  <title>"AAPL" - Google News</title>
  <link>https://news.google.com/search?q=AAPL</link>
  <item>
    <title>Apple hits record high - CNBC</title>
    <link>https://news.google.com/rss/articles/CBMiRECENT?oc=5</link>
    <guid isPermaLink="false">CBMiRECENT</guid>
    <pubDate>Wed, 04 Jun 2025 10:00:00 GMT</pubDate>
    <description><![CDATA[<a href="https://news.google.com/x">Apple hits record high</a> <font>CNBC</font>]]></description>
    <source url="https://www.cnbc.com">CNBC</source>
  </item>
  <item>
    <title>Old Apple story - Reuters</title>
    <link>https://news.google.com/rss/articles/CBMiOLD?oc=5</link>
    <pubDate>Tue, 01 Jan 2019 08:00:00 GMT</pubDate>
    <description>Old news</description>
    <source url="https://www.reuters.com">Reuters</source>
  </item>
  <item>
    <title>No date item</title>
    <link>https://news.google.com/rss/articles/CBMiNODATE?oc=5</link>
    <description>Missing pubDate</description>
    <source url="https://www.bloomberg.com">Bloomberg</source>
  </item>
</channel>
</rss>"#;

    fn sample_channel() -> Channel {
        Channel::read_from(SAMPLE_RSS.as_bytes()).expect("valid RSS")
    }

    fn cutoff(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("valid cutoff")
            .with_timezone(&Utc)
    }

    #[test]
    fn map_articles_keeps_recent_drops_old_and_undated() {
        let articles = map_articles(&sample_channel(), cutoff("2025-01-01T00:00:00Z"));
        assert_eq!(
            articles.len(),
            1,
            "only the recent dated item should survive"
        );
        let a = &articles[0];
        assert_eq!(a.title, "Apple hits record high - CNBC");
        assert_eq!(a.source, "www.cnbc.com");
        assert_eq!(
            a.url.as_deref(),
            Some("https://news.google.com/rss/articles/CBMiRECENT?oc=5")
        );
        assert!(a.snippet.contains("Apple hits record high"));
        assert!(
            !a.snippet.contains('<'),
            "HTML must be stripped: {}",
            a.snippet
        );
        assert!(a.published_at.starts_with("2025-06-04"));
        assert!(a.relevance_score.is_none());
    }

    #[test]
    fn map_articles_empty_when_all_before_cutoff() {
        assert!(map_articles(&sample_channel(), cutoff("2030-01-01T00:00:00Z")).is_empty());
    }

    #[tokio::test]
    async fn fetch_returns_empty_for_non_equity_symbol() {
        use crate::domain::CaipAssetId;
        let json = r#"{"crypto":"eip155:1/slip44:60"}"#;
        let sym: Symbol = serde_json::from_str(json).expect("valid symbol json");
        let provider = GnewsNewsProvider::new();
        let news = provider.fetch(&sym).await.expect("ok");
        assert!(news.articles.is_empty());
        assert!(news.summary.contains("unsupported symbol shape"));
        // Type-guard so unused imports survive.
        let _unused: Option<CaipAssetId> = None;
    }

    // ── HTTP wire test against wiremock ──────────────────────────────────

    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn fetch_issues_single_request_and_parses_feed() {
        let server = MockServer::start().await;
        // Recent pubDate so the article survives the `now - NEWS_ANALYSIS_DAYS`
        // cutoff regardless of when the test runs.
        let recent = (Utc::now() - chrono::Duration::days(1)).to_rfc2822();
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel><title>AAPL - Google News</title>
<link>https://news.google.com</link>
<item><title>Apple does a thing - CNBC</title>
<link>https://news.google.com/rss/articles/ABC?oc=5</link>
<pubDate>{recent}</pubDate>
<description><![CDATA[<p>Apple does a thing</p>]]></description>
<source url="https://www.cnbc.com">CNBC</source></item>
</channel></rss>"#
        );

        Mock::given(method("GET"))
            .and(path("/rss/search"))
            .and(query_param("q", "AAPL"))
            .and(header("user-agent", GNEWS_USER_AGENT))
            // Exactly ONE request — proves the per-article scrape fan-out is gone.
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let provider = GnewsNewsProvider::new().with_base_url(server.uri());
        let symbol = Symbol::parse("AAPL").expect("valid equity symbol");
        let news = provider.fetch(&symbol).await.expect("ok");

        assert_eq!(news.articles.len(), 1);
        assert_eq!(news.articles[0].source, "www.cnbc.com");
        assert!(news.summary.contains("1 article"));
    }

    #[tokio::test]
    async fn fetch_maps_non_success_status_to_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let provider = GnewsNewsProvider::new().with_base_url(server.uri());
        let symbol = Symbol::parse("AAPL").expect("valid equity symbol");
        let err = provider
            .fetch(&symbol)
            .await
            .expect_err("should fail on 503");
        assert!(matches!(err, TradingError::AnalystError { .. }));
    }
}
