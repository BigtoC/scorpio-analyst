//! SEC EDGAR HTTP client.
//!
//! Fetches recent 8-K and 13D/G filings for a given CIK using EDGAR's
//! structured submissions JSON endpoint. All outbound requests carry the
//! hardcoded Scorpio User-Agent per SEC fair-use policy and are gated
//! behind a [`SharedRateLimiter`].
//!
//! ## Fail-soft contract
//!
//! Every public method is fail-soft: it always returns `Ok(...)`.
//! Network errors, HTTP 4xx/5xx, malformed JSON, and unknown tickers all
//! produce `Ok(empty)` with a `tracing::warn!(kind = "catalyst_fetch_failed")`.
//! A per-instance circuit breaker limits retry cost when SEC EDGAR is
//! persistently unavailable.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::{
    data::sec_edgar_nport::NPortHoldings, error::TradingError, rate_limit::SharedRateLimiter,
};

pub mod nport;

/// SEC EDGAR fair-use User-Agent. Hardcoded per policy — no config required.
const SEC_EDGAR_USER_AGENT: &str = "Scorpio Analyst scorpio@ledgerlylab.com";
const EDGAR_DATA_BASE_URL: &str = "https://data.sec.gov";
const EDGAR_WWW_BASE_URL: &str = "https://www.sec.gov";
const COMPANY_TICKERS_PATH: &str = "/files/company_tickers.json";
const SUBMISSIONS_PATH_PREFIX: &str = "/submissions/CIK";

/// Number of consecutive HTTP/transport failures before the circuit opens.
const CIRCUIT_OPEN_THRESHOLD: u32 = 5;
/// How long the breaker stays open before allowing one trial request.
const CIRCUIT_COOLDOWN: Duration = Duration::from_secs(60);

// ─── Public types ─────────────────────────────────────────────────────────────

/// A single filing header returned from the SEC EDGAR submissions index.
#[derive(Debug, Clone, PartialEq)]
pub struct FilingHeader {
    pub cik: u32,
    pub accession_number: String,
    pub form_type: String,
    /// ISO-8601 `YYYY-MM-DD`.
    pub filing_date: String,
    /// Full URL to the primary document on SEC EDGAR.
    pub primary_doc_url: String,
    /// Comma-separated 8-K item codes (e.g. `"1.01,2.02"`). Empty for non-8-K filings.
    pub item_codes: String,
}

// ─── Internal HTTP abstraction (enables unit-test mocking without wiremock) ───

#[cfg_attr(test, mockall::automock)]
#[async_trait]
trait EdgarHttp: Send + Sync {
    /// Execute a GET request and return `(status_code, body_text)`.
    /// Transport-level errors (connection refused, timeout) are returned as `Err`.
    async fn get(&self, url: &str) -> Result<(u16, String), String>;
}

struct ReqwestEdgarHttp {
    client: reqwest::Client,
}

#[async_trait]
impl EdgarHttp for ReqwestEdgarHttp {
    async fn get(&self, url: &str) -> Result<(u16, String), String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(|e| e.to_string())?;
        Ok((status, body))
    }
}

// ─── Circuit breaker ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct CircuitBreakerState {
    consecutive_failures: u32,
    open_until: Option<tokio::time::Instant>,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            open_until: None,
        }
    }

    /// Returns `true` when the breaker is open and the caller should skip the request.
    fn is_open(&self) -> bool {
        match self.open_until {
            Some(until) => tokio::time::Instant::now() < until,
            None => false,
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.open_until = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= CIRCUIT_OPEN_THRESHOLD {
            self.open_until = Some(tokio::time::Instant::now() + CIRCUIT_COOLDOWN);
        }
    }
}

// ─── Deserialization types ────────────────────────────────────────────────────

/// Shape of `https://www.sec.gov/files/company_tickers.json`.
///
/// EDGAR returns an object keyed by ascending integer strings, each holding
/// one ticker record. We collect values only.
#[derive(Deserialize)]
struct CompanyTickerRecord {
    cik_str: u32,
    ticker: String,
}

/// Shape of `https://data.sec.gov/submissions/CIK<10-digit>.json`.
#[derive(Deserialize)]
struct EdgarSubmissionsResponse {
    filings: EdgarFilingsBlock,
}

#[derive(Deserialize)]
struct EdgarFilingsBlock {
    recent: EdgarRecentFilings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EdgarRecentFilings {
    accession_number: Vec<String>,
    filing_date: Vec<String>,
    form: Vec<String>,
    primary_document: Vec<String>,
    /// Item codes — one element per filing. May be an empty string for non-8-K
    /// filings or 8-Ks with no items listed. Field may be absent in older records.
    #[serde(default)]
    items: Vec<String>,
}

// ─── Pure parsing helpers (tested in isolation) ───────────────────────────────

fn parse_company_tickers(body: &str) -> Result<HashMap<String, u32>, String> {
    let raw: HashMap<String, CompanyTickerRecord> =
        serde_json::from_str(body).map_err(|e| e.to_string())?;
    Ok(raw
        .into_values()
        .map(|r| (r.ticker.to_uppercase(), r.cik_str))
        .collect())
}

fn parse_submissions(
    body: &str,
    cik: u32,
    form_types: &[&str],
    from: &str,
    to: &str,
) -> Result<Vec<FilingHeader>, String> {
    let resp: EdgarSubmissionsResponse = serde_json::from_str(body).map_err(|e| e.to_string())?;

    let recent = resp.filings.recent;
    let count = recent.accession_number.len();

    let mut out = Vec::new();
    for i in 0..count {
        let form = recent.form.get(i).map(String::as_str).unwrap_or("");
        if !form_types.iter().any(|&ft| ft.eq_ignore_ascii_case(form)) {
            continue;
        }

        let date = recent.filing_date.get(i).map(String::as_str).unwrap_or("");
        if date < from || date > to {
            continue;
        }

        let accession = recent.accession_number.get(i).cloned().unwrap_or_default();
        let primary_doc = recent.primary_document.get(i).cloned().unwrap_or_default();
        let item_codes = recent
            .items
            .get(i)
            .cloned()
            .unwrap_or_default()
            .replace(", ", ",");

        let accession_no_dashes = accession.replace('-', "");
        let primary_doc_url = format!(
            "{EDGAR_WWW_BASE_URL}/Archives/edgar/data/{cik}/{accession_no_dashes}/{primary_doc}"
        );

        out.push(FilingHeader {
            cik,
            accession_number: accession,
            form_type: form.to_owned(),
            filing_date: date.to_owned(),
            primary_doc_url,
            item_codes,
        });
    }

    Ok(out)
}

// ─── Client ──────────────────────────────────────────────────────────────────

/// Async client for the SEC EDGAR submissions API.
///
/// Fail-soft: all fetch methods return `Ok(...)`. Errors are absorbed and
/// logged with `kind = "catalyst_fetch_failed"`. A circuit breaker prevents
/// repeated network storms when EDGAR is unavailable.
pub struct SecEdgarClient {
    http: Arc<dyn EdgarHttp>,
    limiter: SharedRateLimiter,
    /// Lazy-loaded ticker→CIK map from `company_tickers.json`.
    cik_cache: Arc<RwLock<Option<HashMap<String, u32>>>>,
    breaker: Arc<Mutex<CircuitBreakerState>>,
}

impl std::fmt::Debug for SecEdgarClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecEdgarClient")
            .field("limiter", &self.limiter.label())
            .finish()
    }
}

impl SecEdgarClient {
    /// Construct a client using the hardcoded Scorpio User-Agent.
    ///
    /// Returns `Err` only when `reqwest::Client` fails to build (virtually
    /// impossible in practice, but surfaced so callers can fall back to
    /// `Tier1CatalystProvider` without aborting the pipeline).
    pub fn new(limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(SEC_EDGAR_USER_AGENT)
            .build()
            .map_err(|e| {
                TradingError::Config(anyhow::anyhow!("SEC EDGAR reqwest client build: {e}"))
            })?;

        Ok(Self {
            http: Arc::new(ReqwestEdgarHttp { client }),
            limiter,
            cik_cache: Arc::new(RwLock::new(None)),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::new())),
        })
    }

    #[cfg(test)]
    fn with_http(http: Arc<dyn EdgarHttp>, limiter: SharedRateLimiter) -> Self {
        Self {
            http,
            limiter,
            cik_cache: Arc::new(RwLock::new(None)),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::new())),
        }
    }

    #[cfg(test)]
    fn with_preloaded_cache(
        http: Arc<dyn EdgarHttp>,
        limiter: SharedRateLimiter,
        cache: HashMap<String, u32>,
    ) -> Self {
        Self {
            http,
            limiter,
            cik_cache: Arc::new(RwLock::new(Some(cache))),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::new())),
        }
    }

    /// Look up the CIK for a ticker symbol. Returns `Ok(None)` for unknown tickers.
    ///
    /// The ticker→CIK map is loaded once from EDGAR on first call and cached
    /// for the lifetime of this client.
    pub async fn lookup_cik(&self, ticker: &str) -> Result<Option<u32>, TradingError> {
        let ticker_upper = ticker.to_uppercase();

        {
            let guard = self.cik_cache.read().await;
            if let Some(cache) = guard.as_ref() {
                return Ok(cache.get(&ticker_upper).copied());
            }
        }

        {
            let breaker = self.breaker.lock().await;
            if breaker.is_open() {
                tracing::debug!(
                    ticker,
                    "SEC EDGAR circuit breaker open; skipping CIK lookup"
                );
                return Ok(None);
            }
        }

        let url = format!("{EDGAR_WWW_BASE_URL}{COMPANY_TICKERS_PATH}");
        self.limiter.acquire().await;
        let result = self.http.get(&url).await;

        match result {
            Err(transport_err) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_cik_lookup",
                    ticker,
                    error = %transport_err,
                    "SEC EDGAR company_tickers.json fetch failed"
                );
                Ok(None)
            }
            Ok((status, _)) if status != 200 => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_cik_lookup",
                    ticker,
                    http_status = status,
                    "SEC EDGAR company_tickers.json returned non-200"
                );
                Ok(None)
            }
            Ok((_, body)) => match parse_company_tickers(&body) {
                Err(parse_err) => {
                    self.breaker.lock().await.record_failure();
                    tracing::warn!(
                        kind = "catalyst_fetch_failed",
                        source = "sec_edgar_cik_lookup",
                        ticker,
                        error = %parse_err,
                        "SEC EDGAR company_tickers.json parse failed"
                    );
                    Ok(None)
                }
                Ok(map) => {
                    self.breaker.lock().await.record_success();
                    let cik = map.get(&ticker_upper).copied();
                    *self.cik_cache.write().await = Some(map);
                    Ok(cik)
                }
            },
        }
    }

    /// Fetch recent filings for a CIK, filtered by form type and date range.
    ///
    /// Always returns `Ok(...)`. Network errors, HTTP errors, and parse
    /// failures produce `Ok(vec![])` with a `tracing::warn!`.
    pub async fn fetch_recent_filings(
        &self,
        cik: u32,
        form_types: &[&str],
        from: &str,
        to: &str,
    ) -> Result<Vec<FilingHeader>, TradingError> {
        {
            let breaker = self.breaker.lock().await;
            if breaker.is_open() {
                tracing::debug!(cik, "SEC EDGAR circuit breaker open; skipping fetch");
                return Ok(vec![]);
            }
        }

        let padded_cik = format!("{cik:010}");
        let url = format!("{EDGAR_DATA_BASE_URL}{SUBMISSIONS_PATH_PREFIX}{padded_cik}.json");

        self.limiter.acquire().await;
        let result = self.http.get(&url).await;

        match result {
            Err(transport_err) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar",
                    cik,
                    error = %transport_err,
                    "SEC EDGAR submissions fetch transport error"
                );
                Ok(vec![])
            }
            Ok((429, _)) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar",
                    cik,
                    http_status = 429u16,
                    "SEC EDGAR rate-limited; returning empty filings"
                );
                Ok(vec![])
            }
            Ok((status, _)) if status >= 500 => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar",
                    cik,
                    http_status = status,
                    "SEC EDGAR submissions server error"
                );
                Ok(vec![])
            }
            Ok((status, _)) if status >= 400 => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar",
                    cik,
                    http_status = status,
                    "SEC EDGAR submissions returned error status"
                );
                Ok(vec![])
            }
            Ok((_, body)) => match parse_submissions(&body, cik, form_types, from, to) {
                Err(parse_err) => {
                    self.breaker.lock().await.record_failure();
                    tracing::warn!(
                        kind = "catalyst_fetch_failed",
                        source = "sec_edgar",
                        cik,
                        error = %parse_err,
                        "SEC EDGAR submissions response parse failed"
                    );
                    Ok(vec![])
                }
                Ok(filings) => {
                    self.breaker.lock().await.record_success();
                    Ok(filings)
                }
            },
        }
    }

    /// Resolve a fund ticker to a zero-padded 10-digit CIK string.
    ///
    /// Fail-soft: returns `None` when the underlying [`lookup_cik`] call
    /// fails or the ticker is not in the EDGAR `company_tickers.json` map.
    pub async fn resolve_fund_cik(&self, ticker: &str) -> Option<String> {
        match self.lookup_cik(ticker).await {
            Ok(Some(cik)) => Some(format!("{cik:010}")),
            _ => None,
        }
    }

    /// Fetch the most recent N-PORT-P filing for a fund CIK and parse it.
    ///
    /// Fail-soft: returns `None` on transport errors, non-SEC document URLs,
    /// parse failures, or when no N-PORT-P filing exists within the window
    /// `[today - max_age_days, today]`.
    pub async fn fetch_latest_nport_p(
        &self,
        cik: &str,
        max_age_days: u32,
    ) -> Option<NPortHoldings> {
        let parsed_cik: u32 = cik.trim_start_matches('0').parse().ok()?;
        let today = chrono::Utc::now().date_naive();
        let earliest = today - chrono::Duration::days(max_age_days as i64);
        let filings = self
            .fetch_recent_filings(
                parsed_cik,
                &["NPORT-P"],
                &earliest.to_string(),
                &today.to_string(),
            )
            .await
            .ok()?;
        let latest = filings.first()?;
        if !is_allowed_sec_document_url(&latest.primary_doc_url) {
            tracing::warn!(
                url = %latest.primary_doc_url,
                "skipping non-SEC or non-HTTPS N-PORT document url"
            );
            return None;
        }
        let filing_date =
            chrono::NaiveDate::parse_from_str(&latest.filing_date, "%Y-%m-%d").ok()?;
        let xml = self.fetch_document_text(&latest.primary_doc_url).await?;
        nport::parse_nport_p(&xml, filing_date)
    }

    /// Fetch a raw filing document body. Fail-soft: returns `None` on any
    /// transport error or non-200 response.
    async fn fetch_document_text(&self, url: &str) -> Option<String> {
        {
            let breaker = self.breaker.lock().await;
            if breaker.is_open() {
                tracing::debug!(
                    url,
                    "SEC EDGAR circuit breaker open; skipping document fetch"
                );
                return None;
            }
        }
        self.limiter.acquire().await;
        match self.http.get(url).await {
            Ok((200, body)) => {
                self.breaker.lock().await.record_success();
                Some(body)
            }
            Ok((status, _)) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    url,
                    http_status = status,
                    "non-200 N-PORT-P document fetch"
                );
                None
            }
            Err(e) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    url,
                    error = %e,
                    "N-PORT-P document transport error"
                );
                None
            }
        }
    }
}

/// Allowlist guard: only fetch document bodies from official SEC archives over
/// HTTPS. Protects against scheme/host substitution if `primary_doc_url` is
/// ever derived from untrusted input.
fn is_allowed_sec_document_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && matches!(parsed.host_str(), Some("sec.gov" | "www.sec.gov"))
        && parsed.path().starts_with("/Archives/")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── company_tickers.json parsing ─────────────────────────────────────────

    #[test]
    fn parse_company_tickers_extracts_ticker_to_cik_map() {
        let json = r#"{
            "0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."},
            "1": {"cik_str": 789019, "ticker": "MSFT", "title": "Microsoft Corp"}
        }"#;

        let map = parse_company_tickers(json).expect("parse should succeed");
        assert_eq!(map.get("AAPL"), Some(&320193u32));
        assert_eq!(map.get("MSFT"), Some(&789019u32));
    }

    #[test]
    fn parse_company_tickers_normalizes_ticker_to_uppercase() {
        let json = r#"{"0": {"cik_str": 1234, "ticker": "aapl", "title": "Test"}}"#;
        let map = parse_company_tickers(json).expect("parse");
        assert_eq!(map.get("AAPL"), Some(&1234u32));
        assert!(!map.contains_key("aapl"));
    }

    #[test]
    fn parse_company_tickers_rejects_malformed_json() {
        let result = parse_company_tickers("{not-json");
        assert!(result.is_err());
    }

    // ── submissions JSON parsing ──────────────────────────────────────────────

    fn minimal_submissions_json(
        form: &str,
        date: &str,
        accession: &str,
        primary_doc: &str,
        items: Option<&str>,
    ) -> String {
        let items_field = match items {
            Some(items) => format!(r#", "items": ["{items}"]"#),
            None => String::new(),
        };
        format!(
            r#"{{
                "cik": "0000320193",
                "name": "Apple Inc.",
                "filings": {{
                    "recent": {{
                        "accessionNumber": ["{accession}"],
                        "filingDate": ["{date}"],
                        "form": ["{form}"],
                        "primaryDocument": ["{primary_doc}"]{items_field}
                    }}
                }}
            }}"#
        )
    }

    #[test]
    fn parse_submissions_filters_by_form_type() {
        let json = minimal_submissions_json(
            "10-K",
            "2026-01-15",
            "0000320193-26-000123",
            "d123.htm",
            Some(""),
        );
        let filings =
            parse_submissions(&json, 320193, &["8-K"], "2025-01-01", "2027-01-01").expect("parse");
        assert!(
            filings.is_empty(),
            "10-K should be filtered out when only 8-K requested"
        );
    }

    #[test]
    fn parse_submissions_filters_by_date_range() {
        let json = minimal_submissions_json(
            "8-K",
            "2024-01-01",
            "0000320193-24-000001",
            "d24.htm",
            Some("2.02"),
        );
        let filings =
            parse_submissions(&json, 320193, &["8-K"], "2025-01-01", "2027-01-01").expect("parse");
        assert!(
            filings.is_empty(),
            "filing before window should be filtered out"
        );
    }

    #[test]
    fn parse_submissions_happy_path_maps_all_fields() {
        let json = minimal_submissions_json(
            "8-K",
            "2026-01-15",
            "0000320193-26-000123",
            "d8k.htm",
            Some("2.02"),
        );
        let filings =
            parse_submissions(&json, 320193, &["8-K"], "2025-01-01", "2027-01-01").expect("parse");
        assert_eq!(filings.len(), 1);
        let f = &filings[0];
        assert_eq!(f.cik, 320193);
        assert_eq!(f.accession_number, "0000320193-26-000123");
        assert_eq!(f.form_type, "8-K");
        assert_eq!(f.filing_date, "2026-01-15");
        assert_eq!(f.item_codes, "2.02");
        assert!(
            f.primary_doc_url.contains("000032019326000123"),
            "accession number must be stripped of dashes: {}",
            f.primary_doc_url
        );
        assert!(f.primary_doc_url.contains("d8k.htm"));
    }

    #[test]
    fn parse_submissions_missing_items_field_produces_empty_item_codes() {
        let json = minimal_submissions_json(
            "8-K",
            "2026-02-01",
            "0000320193-26-000456",
            "d8k2.htm",
            None, // no `items` field at all
        );
        let filings =
            parse_submissions(&json, 320193, &["8-K"], "2025-01-01", "2027-01-01").expect("parse");
        assert_eq!(filings.len(), 1);
        assert_eq!(
            filings[0].item_codes, "",
            "missing items field should yield empty string"
        );
    }

    #[test]
    fn parse_submissions_normalizes_multi_item_comma_space_separators() {
        let json = minimal_submissions_json(
            "8-K",
            "2026-02-01",
            "0000320193-26-000789",
            "d8k3.htm",
            Some("1.01, 2.01"),
        );
        let filings =
            parse_submissions(&json, 320193, &["8-K"], "2025-01-01", "2027-01-01").expect("parse");
        assert_eq!(filings[0].item_codes, "1.01,2.01");
    }

    #[test]
    fn parse_submissions_rejects_malformed_json() {
        let result = parse_submissions("{bad", 1, &["8-K"], "2025-01-01", "2026-01-01");
        assert!(result.is_err());
    }

    // ── CIK lookup via mock HTTP ──────────────────────────────────────────────

    #[tokio::test]
    async fn lookup_cik_returns_none_for_unknown_ticker() {
        let json = r#"{"0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."}}"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, json.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let cik = client
            .lookup_cik("ZZZNOTREAL")
            .await
            .expect("must not error");
        assert_eq!(cik, None);
    }

    #[tokio::test]
    async fn lookup_cik_returns_some_for_known_ticker() {
        let json = r#"{"0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."}}"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, json.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let cik = client.lookup_cik("AAPL").await.expect("must not error");
        assert_eq!(cik, Some(320193));
    }

    #[tokio::test]
    async fn lookup_cik_is_case_insensitive() {
        let json = r#"{"0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."}}"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, json.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        assert_eq!(client.lookup_cik("aapl").await.unwrap(), Some(320193));
    }

    // ── fetch_recent_filings HTTP error scenarios ─────────────────────────────

    async fn client_with_status(status: u16) -> SecEdgarClient {
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((status, String::new())));
        SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100))
    }

    #[tokio::test]
    async fn fetch_recent_filings_http_403_returns_ok_empty() {
        let client = client_with_status(403).await;
        let result = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(result.is_empty(), "HTTP 403 must yield empty filings");
    }

    #[tokio::test]
    async fn fetch_recent_filings_http_404_returns_ok_empty() {
        let client = client_with_status(404).await;
        let result = client
            .fetch_recent_filings(99_999_999, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(
            result.is_empty(),
            "HTTP 404 (bogus CIK) must yield empty filings"
        );
    }

    #[tokio::test]
    async fn fetch_recent_filings_http_429_returns_ok_empty() {
        let client = client_with_status(429).await;
        let result = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(result.is_empty(), "HTTP 429 must yield empty filings");
    }

    #[tokio::test]
    async fn fetch_recent_filings_http_500_returns_ok_empty() {
        let client = client_with_status(500).await;
        let result = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(result.is_empty(), "HTTP 500 must yield empty filings");
    }

    #[tokio::test]
    async fn fetch_recent_filings_transport_error_returns_ok_empty() {
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(|_url| Err("connection refused".to_owned()));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let result = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(
            result.is_empty(),
            "transport error must yield empty filings"
        );
    }

    #[tokio::test]
    async fn fetch_recent_filings_malformed_json_returns_ok_empty() {
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(|_url| Ok((200, "{not-json}".to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let result = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(
            result.is_empty(),
            "malformed JSON body must yield empty filings"
        );
    }

    #[tokio::test]
    async fn fetch_recent_filings_unknown_cik_via_preloaded_cache_yields_empty() {
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(|_url| Ok((404, String::new())));

        let cache: HashMap<String, u32> = HashMap::new();
        let client = SecEdgarClient::with_preloaded_cache(
            Arc::new(mock_http),
            SharedRateLimiter::new("test", 100),
            cache,
        );
        let cik = client
            .lookup_cik("ZZZNOTREAL")
            .await
            .expect("must not error");
        assert_eq!(cik, None, "unknown ticker must return None");
    }

    // ── Circuit breaker ───────────────────────────────────────────────────────

    #[test]
    fn circuit_breaker_opens_after_threshold_failures() {
        let mut state = CircuitBreakerState::new();
        assert!(!state.is_open());

        for _ in 0..CIRCUIT_OPEN_THRESHOLD {
            state.record_failure();
        }
        assert!(
            state.is_open(),
            "breaker must open after {CIRCUIT_OPEN_THRESHOLD} failures"
        );
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut state = CircuitBreakerState::new();
        for _ in 0..CIRCUIT_OPEN_THRESHOLD - 1 {
            state.record_failure();
        }
        state.record_success();
        assert!(!state.is_open());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn fetch_recent_filings_skips_request_when_circuit_open() {
        // Open the breaker manually.
        let mut mock_http = MockEdgarHttp::new();
        // The mock should be called exactly once for the final successful request
        // that triggers the breaker — but we want to verify subsequent calls are skipped.
        // Set up to return error every time (so breaker opens).
        mock_http
            .expect_get()
            .times(CIRCUIT_OPEN_THRESHOLD as usize)
            .returning(|_| Err("error".to_owned()));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));

        // Exhaust threshold
        for _ in 0..CIRCUIT_OPEN_THRESHOLD {
            let _ = client
                .fetch_recent_filings(1, &["8-K"], "2025-01-01", "2026-12-31")
                .await;
        }

        // This call should be skipped (mock has no more allowed calls).
        let result = client
            .fetch_recent_filings(1, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn lookup_cik_skips_request_when_circuit_open() {
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .times(CIRCUIT_OPEN_THRESHOLD as usize)
            .returning(|_| Err("error".to_owned()));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));

        for _ in 0..CIRCUIT_OPEN_THRESHOLD {
            let _ = client.lookup_cik("SPY").await.expect("must not error");
        }

        let result = client.lookup_cik("SPY").await.expect("must not error");
        assert_eq!(result, None);
    }

    // ── new() construction ────────────────────────────────────────────────────

    #[test]
    fn new_constructs_successfully_with_valid_limiter() {
        let limiter = SharedRateLimiter::new("test-edgar", 10);
        let result = SecEdgarClient::new(limiter);
        assert!(
            result.is_ok(),
            "hardcoded UA should always produce a valid client"
        );
    }

    // ── Live tests (require internet, not run in CI) ──────────────────────────

    #[tokio::test]
    #[ignore = "requires live SEC EDGAR connection — run manually"]
    async fn live_lookup_cik_aapl_returns_known_cik() {
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5)).expect("client");
        let cik = client.lookup_cik("AAPL").await.expect("lookup");
        assert_eq!(cik, Some(320193));
    }

    #[tokio::test]
    #[ignore = "requires live SEC EDGAR connection — run manually"]
    async fn live_fetch_recent_8k_filings_for_aapl_returns_nonempty() {
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5)).expect("client");
        let filings = client
            .fetch_recent_filings(320193, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("fetch");
        assert!(
            !filings.is_empty(),
            "AAPL should have 8-K filings in 2025-2026"
        );
    }

    #[tokio::test]
    #[ignore = "requires live SEC EDGAR connection — run manually"]
    async fn live_fetch_bogus_cik_returns_ok_empty() {
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5)).expect("client");
        let filings = client
            .fetch_recent_filings(99_999_999, &["8-K"], "2025-01-01", "2026-12-31")
            .await
            .expect("must return Ok, not Err");
        assert!(filings.is_empty());
    }

    // ── N-PORT-P resolution + fetch ──────────────────────────────────────────

    #[tokio::test]
    async fn resolve_fund_cik_returns_zero_padded_string_for_known_ticker() {
        let json =
            r#"{"0": {"cik_str": 884394, "ticker": "SPY", "title": "SPDR S&P 500 ETF Trust"}}"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, json.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let cik = client.resolve_fund_cik("SPY").await;
        assert_eq!(cik.as_deref(), Some("0000884394"));
    }

    #[tokio::test]
    async fn resolve_fund_cik_returns_none_for_unknown_ticker() {
        let json = r#"{"0": {"cik_str": 884394, "ticker": "SPY", "title": "SPDR"}}"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, json.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let cik = client.resolve_fund_cik("ZZZNOTREAL").await;
        assert!(cik.is_none());
    }

    #[tokio::test]
    async fn fetch_latest_nport_p_returns_none_when_no_filings_in_window() {
        // submissions response with one filing well outside the requested window.
        let body = r#"{
            "cik": "0000884394",
            "filings": {
                "recent": {
                    "accessionNumber": ["0000884394-20-000001"],
                    "filingDate": ["2020-01-01"],
                    "form": ["NPORT-P"],
                    "primaryDocument": ["primary_doc.xml"]
                }
            }
        }"#;
        let mut mock_http = MockEdgarHttp::new();
        mock_http
            .expect_get()
            .returning(move |_url| Ok((200, body.to_owned())));

        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        // max_age_days = 90 → 2020-01-01 is far outside the window from "today".
        let result = client.fetch_latest_nport_p("0000884394", 90).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn fetch_latest_nport_p_returns_none_for_unparseable_cik() {
        let mock_http = MockEdgarHttp::new();
        let client =
            SecEdgarClient::with_http(Arc::new(mock_http), SharedRateLimiter::new("test", 100));
        let result = client.fetch_latest_nport_p("not-a-number", 90).await;
        assert!(result.is_none());
    }

    // ── URL allowlist ────────────────────────────────────────────────────────

    #[test]
    fn allowed_sec_document_url_accepts_canonical_archives_url() {
        let url =
            "https://www.sec.gov/Archives/edgar/data/884394/000088439426000123/primary_doc.xml";
        assert!(is_allowed_sec_document_url(url));
    }

    #[test]
    fn allowed_sec_document_url_accepts_data_subdomain_when_archives_path() {
        // data.sec.gov is not in the allowlist; only sec.gov / www.sec.gov.
        let url = "https://data.sec.gov/Archives/edgar/data/884394/foo.xml";
        assert!(!is_allowed_sec_document_url(url));
    }

    #[test]
    fn allowed_sec_document_url_rejects_http_scheme() {
        let url =
            "http://www.sec.gov/Archives/edgar/data/884394/000088439426000123/primary_doc.xml";
        assert!(!is_allowed_sec_document_url(url));
    }

    #[test]
    fn allowed_sec_document_url_rejects_foreign_host() {
        let url = "https://evil.example.com/Archives/edgar/data/884394/primary_doc.xml";
        assert!(!is_allowed_sec_document_url(url));
    }

    #[test]
    fn allowed_sec_document_url_rejects_non_archives_path() {
        let url = "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany";
        assert!(!is_allowed_sec_document_url(url));
    }

    #[test]
    fn allowed_sec_document_url_rejects_garbage() {
        assert!(!is_allowed_sec_document_url("not-a-url"));
        assert!(!is_allowed_sec_document_url(""));
    }
}
