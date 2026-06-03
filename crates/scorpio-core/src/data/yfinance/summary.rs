//! Yahoo Finance `quoteSummary` HTTP fetcher for ETF NAV / bid / ask.
//!
//! The upstream `yfinance-rs` 0.7 `Quote`/`Info` types do not expose
//! `navPrice`, `bid`, or `ask`. This module hits Yahoo's v10
//! `quoteSummary` endpoint directly via `reqwest`, mirroring the
//! cookie/crumb authentication pattern that `yfinance-rs` itself uses
//! internally (visit `fc.yahoo.com` → extract Set-Cookie → fetch
//! `getcrumb` → reuse the cookie + crumb on subsequent calls).
//!
//! All failures are soft: any error returns `None` and emits a
//! `tracing::warn`. The caller (`get_quote`) treats this as best-effort
//! enrichment.
//!
//! # Why manual cookie handling
//!
//! Workspace `reqwest` is built without the `cookies` feature today, so
//! this module manually extracts the first `Set-Cookie` header value and
//! attaches it via a `Cookie` header on subsequent requests — the same
//! degree of state the auth flow needs.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use reqwest::header::{COOKIE, SET_COOKIE};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::warn;

const COOKIE_URL: &str = "https://fc.yahoo.com";
const CRUMB_URL: &str = "https://query1.finance.yahoo.com/v1/test/getcrumb";
const SUMMARY_BASE: &str = "https://query2.finance.yahoo.com/v10/finance/quoteSummary";
// Realistic UA — Yahoo rejects requests whose UA looks like a bot.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 \
     (KHTML, like Gecko) Version/17.5 Safari/605.1.15";

/// ETF-specific fields lifted from the `summaryDetail` module of
/// Yahoo's `quoteSummary` response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EtfSummary {
    pub nav: Option<f64>,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct AuthState {
    cookie: Option<String>,
    crumb: Option<String>,
}

/// Internal HTTP client for Yahoo Finance `quoteSummary` calls.
///
/// Cookie + crumb are acquired lazily on first call and cached for the
/// lifetime of the session. A 401/403 response triggers a one-shot
/// re-acquisition and retry.
#[derive(Clone, Debug)]
pub(super) struct SummaryHttp {
    client: Client,
    auth: Arc<RwLock<AuthState>>,
}

impl SummaryHttp {
    pub(super) fn new() -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: Arc::new(RwLock::new(AuthState::default())),
        }
    }

    /// Fetch NAV/bid/ask for `symbol`. Returns `None` on any failure.
    pub(super) async fn fetch(&self, symbol: &str) -> Option<EtfSummary> {
        match self.fetch_once(symbol).await {
            Ok(s) => Some(s),
            Err(FetchError::Unauthorized) => {
                // Crumb may have expired — clear and retry once.
                {
                    let mut auth = self.auth.write().await;
                    auth.crumb = None;
                    auth.cookie = None;
                }
                match self.fetch_once(symbol).await {
                    Ok(s) => Some(s),
                    Err(e) => {
                        warn!(symbol, error = %e, "yahoo quoteSummary retry failed");
                        None
                    }
                }
            }
            Err(e) => {
                warn!(symbol, error = %e, "yahoo quoteSummary fetch failed");
                None
            }
        }
    }

    async fn fetch_once(&self, symbol: &str) -> Result<EtfSummary, FetchError> {
        let (cookie, crumb) = self.ensure_credentials().await?;

        let url = format!("{SUMMARY_BASE}/{symbol}");
        let resp = self
            .client
            .get(url)
            .header(COOKIE, cookie)
            .query(&[("modules", "summaryDetail"), ("crumb", crumb.as_str())])
            .send()
            .await
            .map_err(FetchError::from_reqwest)?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::Unauthorized);
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }

        let body = resp.text().await.map_err(FetchError::from_reqwest)?;
        parse_summary_response(&body).ok_or(FetchError::ParseFailed)
    }

    /// Acquire (cookie, crumb) — cached after first success.
    async fn ensure_credentials(&self) -> Result<(String, String), FetchError> {
        {
            let read = self.auth.read().await;
            if let (Some(c), Some(k)) = (&read.cookie, &read.crumb) {
                return Ok((c.clone(), k.clone()));
            }
        }

        // Fetch cookie from fc.yahoo.com.
        let cookie_resp = self
            .client
            .get(COOKIE_URL)
            .send()
            .await
            .map_err(FetchError::from_reqwest)?;
        let cookie_header = cookie_resp
            .headers()
            .get(SET_COOKIE)
            .and_then(|h| h.to_str().ok())
            .map(|s| {
                // Keep only `name=value` (drop attributes after the first `;`).
                s.split(';').next().unwrap_or("").trim().to_owned()
            })
            .filter(|s| !s.is_empty())
            .ok_or(FetchError::NoCookie)?;

        // Fetch crumb using the cookie.
        let crumb_resp = self
            .client
            .get(CRUMB_URL)
            .header(COOKIE, &cookie_header)
            .send()
            .await
            .map_err(FetchError::from_reqwest)?;
        if !crumb_resp.status().is_success() {
            return Err(FetchError::Status(crumb_resp.status().as_u16()));
        }
        let crumb = crumb_resp
            .text()
            .await
            .map_err(FetchError::from_reqwest)?
            .trim()
            .to_owned();
        if crumb.is_empty() || crumb.contains('{') || crumb.contains('<') {
            return Err(FetchError::BadCrumb);
        }

        {
            let mut write = self.auth.write().await;
            write.cookie = Some(cookie_header.clone());
            write.crumb = Some(crumb.clone());
        }
        Ok((cookie_header, crumb))
    }
}

#[derive(Debug)]
enum FetchError {
    Reqwest(String),
    Status(u16),
    Unauthorized,
    NoCookie,
    BadCrumb,
    ParseFailed,
}

impl FetchError {
    fn from_reqwest(e: reqwest::Error) -> Self {
        Self::Reqwest(e.to_string())
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reqwest(e) => write!(f, "http error: {e}"),
            Self::Status(s) => write!(f, "non-success status: {s}"),
            Self::Unauthorized => write!(f, "unauthorized (crumb expired?)"),
            Self::NoCookie => write!(f, "no Set-Cookie on fc.yahoo.com"),
            Self::BadCrumb => write!(f, "received invalid crumb"),
            Self::ParseFailed => write!(f, "could not extract summaryDetail"),
        }
    }
}

// ── JSON shapes ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteSummaryResponse {
    quote_summary: QuoteSummary,
}

#[derive(Deserialize)]
struct QuoteSummary {
    result: Option<Vec<QuoteResult>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteResult {
    summary_detail: Option<SummaryDetail>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SummaryDetail {
    bid: Option<RawValue>,
    ask: Option<RawValue>,
    nav_price: Option<RawValue>,
}

#[derive(Deserialize)]
struct RawValue {
    raw: Option<f64>,
}

/// Parse a `quoteSummary` JSON body into `EtfSummary`. Pure function —
/// no network. Returns `None` when the envelope cannot be deserialized
/// or no result is present.
#[must_use]
fn parse_summary_response(body: &str) -> Option<EtfSummary> {
    let envelope: QuoteSummaryResponse = serde_json::from_str(body).ok()?;
    let detail = envelope
        .quote_summary
        .result?
        .into_iter()
        .next()?
        .summary_detail?;
    Some(EtfSummary {
        nav: detail.nav_price.and_then(|v| v.raw),
        bid: detail.bid.and_then(|v| v.raw),
        ask: detail.ask.and_then(|v| v.raw),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_payload() {
        let body = r#"{
            "quoteSummary": {
                "result": [{
                    "summaryDetail": {
                        "bid": {"raw": 620.50, "fmt": "620.50"},
                        "ask": {"raw": 620.52, "fmt": "620.52"},
                        "navPrice": {"raw": 620.30, "fmt": "620.30"}
                    }
                }],
                "error": null
            }
        }"#;
        let s = parse_summary_response(body).expect("parses");
        assert_eq!(s.nav, Some(620.30));
        assert_eq!(s.bid, Some(620.50));
        assert_eq!(s.ask, Some(620.52));
    }

    #[test]
    fn parses_partial_payload_missing_nav() {
        let body = r#"{
            "quoteSummary": {
                "result": [{
                    "summaryDetail": {
                        "bid": {"raw": 12.34},
                        "ask": {"raw": 12.35}
                    }
                }]
            }
        }"#;
        let s = parse_summary_response(body).expect("parses");
        assert_eq!(s.nav, None);
        assert_eq!(s.bid, Some(12.34));
        assert_eq!(s.ask, Some(12.35));
    }

    #[test]
    fn parses_empty_result_returns_none() {
        let body = r#"{"quoteSummary":{"result":[],"error":null}}"#;
        assert!(parse_summary_response(body).is_none());
    }

    #[test]
    fn parses_null_result_returns_none() {
        let body = r#"{"quoteSummary":{"result":null,"error":{"description":"x"}}}"#;
        assert!(parse_summary_response(body).is_none());
    }

    #[test]
    fn parses_missing_summary_detail_returns_none() {
        let body = r#"{"quoteSummary":{"result":[{}]}}"#;
        assert!(parse_summary_response(body).is_none());
    }

    #[test]
    fn parses_garbage_returns_none() {
        assert!(parse_summary_response("not json").is_none());
        assert!(parse_summary_response("").is_none());
    }
}
