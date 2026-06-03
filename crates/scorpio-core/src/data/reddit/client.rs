//! HTTP wrapper around Reddit's anonymous `search.json` endpoint.
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
    ///
    /// Produces `{base}/r/<sub1+sub2+...>/search.json?q=<q>&restrict_sr=on&sort=new&over_18=false&stickied=false&limit=<n>`.
    ///
    /// The `over_18=false` and `stickied=false` parameters are server-side
    /// hints; the provider applies defensive client-side filters too.
    pub(crate) fn build_search_url(
        &self,
        subreddits: &[String],
        query: &str,
        limit: u32,
    ) -> String {
        let joined = subreddits.join("+");
        let encoded_q = url_encode(query);
        format!(
            "{base}/r/{subs}/search.json?q={q}&restrict_sr=on&sort=new&over_18=false&stickied=false&limit={limit}",
            base = self.base_url.trim_end_matches('/'),
            subs = joined,
            q = encoded_q,
            limit = limit,
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
            return Err(TradingError::AnalystError {
                agent: "reddit".to_owned(),
                message: "reddit: access denied (HTTP 403) — anonymous API access may be restricted".to_owned(),
            });
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
    async fn forbidden_403_maps_to_analyst_error_with_access_denied_message() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let err = client_against(&server)
            .search_submissions(&["stocks".to_owned()], "AAPL", 100)
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, TradingError::AnalystError { ref agent, .. } if agent == "reddit"),
            "expected AnalystError(reddit), got: {err:?}"
        );
        assert!(
            format!("{err}").contains("access denied"),
            "error message should mention access denied, got: {err}"
        );
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
