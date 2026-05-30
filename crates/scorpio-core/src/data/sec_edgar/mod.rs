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
/// Mutual-fund / ETF ticker index. SEC publishes ETFs here (as fund series),
/// **not** in `company_tickers.json` — that file is operating companies only.
const COMPANY_TICKERS_MF_PATH: &str = "/files/company_tickers_mf.json";
const SUBMISSIONS_PATH_PREFIX: &str = "/submissions/CIK";

/// EDGAR browse-edgar endpoint — series-aware filing index.
///
/// Multi-series fund trusts (e.g. iShares Trust CIK 1100663 holds ~100 ETFs)
/// publish one N-PORT-P per series under the trust CIK. The submissions
/// endpoint returns them all undifferentiated. The full-text search endpoint
/// at `efts.sec.gov` does **not** support series filtering (the `series=`
/// query parameter is silently ignored). The legacy `browse-edgar` endpoint
/// does support series filtering by accepting a series identifier where a
/// CIK normally goes, and returns an Atom feed listing filings specific to
/// that series.
const EDGAR_BROWSE_PATH: &str = "/cgi-bin/browse-edgar";

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

/// Shape of `https://www.sec.gov/files/company_tickers_mf.json`.
///
/// SEC returns a column-oriented layout:
/// ```json
/// { "fields": ["cik", "seriesId", "classId", "symbol"],
///   "data":   [[1100663, "S000004354", "C000012084", "SOXX"], ...] }
/// ```
/// We deserialize each row as a fixed-arity tuple and rely on the documented
/// column order. If SEC ever reorders the columns, parsing succeeds but the
/// resulting CIKs will be wrong — guarded against in [`parse_company_tickers_mf`]
/// by checking the `fields` header.
#[derive(Deserialize)]
struct MfTickersResponse {
    fields: Vec<String>,
    data: Vec<(u32, String, String, String)>,
}

/// Resolved MF/ETF ticker record — CIK identifies the parent trust, series
/// identifies the specific fund within that trust.
///
/// For multi-series trusts (iShares Trust, Vanguard Group, etc.), `cik` alone
/// is not enough to isolate a specific ETF's N-PORT-P filing because the
/// trust files one per series under the same CIK. The `series_id` is the
/// missing key that EDGAR's full-text search uses to filter filings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfTickerEntry {
    pub cik: u32,
    /// EDGAR series identifier, e.g. `"S000004354"` for SOXX.
    pub series_id: String,
    /// EDGAR class identifier, e.g. `"C000012084"` for SOXX — load-bearing for
    /// the SEC risk/return benchmark lookup.
    pub class_id: String,
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

/// Parse the MF/ETF tickers JSON into a `ticker → MfTickerEntry` map.
///
/// Defends against silent column reordering by checking the `fields` header
/// — the parser requires `cik` at index 0, `seriesId` at index 1, and
/// `symbol` at index 3. If SEC ever changes the schema, we fail loudly rather
/// than emit wrong CIK/series-ID pairs.
fn parse_company_tickers_mf(body: &str) -> Result<HashMap<String, MfTickerEntry>, String> {
    let raw: MfTickersResponse = serde_json::from_str(body).map_err(|e| e.to_string())?;
    if raw.fields.first().map(String::as_str) != Some("cik")
        || raw.fields.get(1).map(String::as_str) != Some("seriesId")
        || raw.fields.get(2).map(String::as_str) != Some("classId")
        || raw.fields.get(3).map(String::as_str) != Some("symbol")
    {
        return Err(format!(
            "unexpected company_tickers_mf.json schema: {:?}",
            raw.fields
        ));
    }
    Ok(raw
        .data
        .into_iter()
        .map(|(cik, series_id, class_id, symbol)| {
            (
                symbol.to_uppercase(),
                MfTickerEntry {
                    cik,
                    series_id,
                    class_id,
                },
            )
        })
        .collect())
}

/// Parse a `browse-edgar` Atom feed into ordered `FilingHeader` rows.
///
/// SEC's `browse-edgar` Atom output lists filings most-recent-first. Each
/// `<entry>` carries a nested `<content>` block with `<accession-number>`,
/// `<filing-date>`, and `<filing-type>` children. The owning CIK is passed
/// in by the caller (it's the trust CIK we already have from
/// `MfTickerEntry`), avoiding any dependency on parsing the `<filing-href>`
/// URL.
///
/// Constructs the primary document URL using the standard N-PORT-P
/// convention (`primary_doc.xml`); SEC has filed every N-PORT-P with this
/// fixed filename since the form was introduced.
fn parse_browse_edgar_atom(
    body: &str,
    owner_cik: u32,
    form_filter: &str,
    from: &str,
    to: &str,
) -> Result<Vec<FilingHeader>, String> {
    let mut reader = quick_xml::reader::Reader::from_str(body);
    reader.config_mut().trim_text(true);

    let mut out: Vec<FilingHeader> = Vec::new();
    let mut in_entry = false;
    let mut accession: Option<String> = None;
    let mut filing_date: Option<String> = None;
    let mut filing_type: Option<String> = None;
    let mut current_field: Option<Vec<u8>> = None;
    let mut current_text = String::new();

    loop {
        match reader.read_event() {
            Err(e) => return Err(format!("atom parse: {e}")),
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = e.name().as_ref().to_vec();
                match name.as_slice() {
                    b"entry" => {
                        in_entry = true;
                        accession = None;
                        filing_date = None;
                        filing_type = None;
                    }
                    b"accession-number" | b"filing-date" | b"filing-type" if in_entry => {
                        current_field = Some(name);
                        current_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::Text(t)) => {
                // `decode()` replaces `unescape()` (removed in quick-xml 0.38);
                // entity references now arrive as separate `GeneralRef` events.
                if current_field.is_some()
                    && let Ok(s) = t.decode()
                {
                    current_text.push_str(&s);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) => {
                if current_field.is_some()
                    && let Some(ch) = nport::resolve_general_ref(&r)
                {
                    current_text.push(ch);
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let name = e.name().as_ref().to_vec();
                if let Some(field) = current_field.as_ref()
                    && field == &name
                {
                    let value = current_text.trim().to_owned();
                    match name.as_slice() {
                        b"accession-number" => accession = Some(value),
                        b"filing-date" => filing_date = Some(value),
                        b"filing-type" => filing_type = Some(value),
                        _ => {}
                    }
                    current_field = None;
                }
                if name == b"entry" {
                    in_entry = false;
                    if let (Some(acc), Some(date), Some(form)) =
                        (accession.take(), filing_date.take(), filing_type.take())
                        && form.eq_ignore_ascii_case(form_filter)
                        && date.as_str() >= from
                        && date.as_str() <= to
                    {
                        let accession_no_dashes = acc.replace('-', "");
                        let primary_doc_url = format!(
                            "{EDGAR_WWW_BASE_URL}/Archives/edgar/data/{owner_cik}/{accession_no_dashes}/primary_doc.xml"
                        );
                        out.push(FilingHeader {
                            cik: owner_cik,
                            accession_number: acc,
                            form_type: form,
                            filing_date: date,
                            primary_doc_url,
                            item_codes: String::new(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    Ok(out)
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
    /// Lazy-loaded ticker→CIK map from `company_tickers.json` (operating
    /// companies only — equities filing 10-K/10-Q).
    cik_cache: Arc<RwLock<Option<HashMap<String, u32>>>>,
    /// Lazy-loaded ticker→`MfTickerEntry` map from `company_tickers_mf.json`
    /// (ETFs and mutual fund series). Populated only on first ETF miss in
    /// [`Self::resolve_fund_cik`] or [`Self::fetch_latest_nport_p_for_ticker`].
    /// The series ID is kept alongside the CIK so multi-series trusts can be
    /// resolved to a specific fund.
    cik_mf_cache: Arc<RwLock<Option<HashMap<String, MfTickerEntry>>>>,
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
    /// Infallible: `reqwest::Client::builder().build()` can only fail when the
    /// system TLS backend cannot be initialized — virtually impossible in
    /// practice and equally fatal to every other HTTP client in the process —
    /// so the rare failure degrades to `reqwest::Client::new()` (the same
    /// default builder, which itself panics on a broken TLS stack). Callers no
    /// longer thread a `Result` through pipeline construction.
    pub fn new(limiter: SharedRateLimiter) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(SEC_EDGAR_USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http: Arc::new(ReqwestEdgarHttp { client }),
            limiter,
            cik_cache: Arc::new(RwLock::new(None)),
            cik_mf_cache: Arc::new(RwLock::new(None)),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::new())),
        }
    }

    #[cfg(test)]
    fn with_http(http: Arc<dyn EdgarHttp>, limiter: SharedRateLimiter) -> Self {
        Self {
            http,
            limiter,
            cik_cache: Arc::new(RwLock::new(None)),
            cik_mf_cache: Arc::new(RwLock::new(None)),
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
            cik_mf_cache: Arc::new(RwLock::new(None)),
            breaker: Arc::new(Mutex::new(CircuitBreakerState::new())),
        }
    }

    #[cfg(test)]
    fn with_preloaded_mf_cache(
        http: Arc<dyn EdgarHttp>,
        limiter: SharedRateLimiter,
        mf_cache: HashMap<String, MfTickerEntry>,
    ) -> Self {
        Self {
            http,
            limiter,
            cik_cache: Arc::new(RwLock::new(None)),
            cik_mf_cache: Arc::new(RwLock::new(Some(mf_cache))),
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

    /// Look up the full MF ticker entry (CIK + series ID) for a ticker in
    /// EDGAR's MF/ETF ticker index.
    ///
    /// Used by [`Self::resolve_fund_cik`] as a fallback when the operating-
    /// company map ([`Self::lookup_cik`]) misses, and by
    /// [`Self::fetch_latest_nport_p_for_ticker`] to obtain the series ID
    /// needed to disambiguate multi-series trusts. Same fail-soft contract:
    /// `Ok(None)` on transport error, non-200 status, or unknown ticker.
    pub async fn lookup_cik_mf(&self, ticker: &str) -> Result<Option<MfTickerEntry>, TradingError> {
        let ticker_upper = ticker.to_uppercase();

        {
            let guard = self.cik_mf_cache.read().await;
            if let Some(cache) = guard.as_ref() {
                return Ok(cache.get(&ticker_upper).cloned());
            }
        }

        {
            let breaker = self.breaker.lock().await;
            if breaker.is_open() {
                tracing::debug!(
                    ticker,
                    "SEC EDGAR circuit breaker open; skipping MF CIK lookup"
                );
                return Ok(None);
            }
        }

        let url = format!("{EDGAR_WWW_BASE_URL}{COMPANY_TICKERS_MF_PATH}");
        self.limiter.acquire().await;
        let result = self.http.get(&url).await;

        match result {
            Err(transport_err) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_cik_mf_lookup",
                    ticker,
                    error = %transport_err,
                    "SEC EDGAR company_tickers_mf.json fetch failed"
                );
                Ok(None)
            }
            Ok((status, _)) if status != 200 => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_cik_mf_lookup",
                    ticker,
                    http_status = status,
                    "SEC EDGAR company_tickers_mf.json returned non-200"
                );
                Ok(None)
            }
            Ok((_, body)) => match parse_company_tickers_mf(&body) {
                Err(parse_err) => {
                    self.breaker.lock().await.record_failure();
                    tracing::warn!(
                        kind = "catalyst_fetch_failed",
                        source = "sec_edgar_cik_mf_lookup",
                        ticker,
                        error = %parse_err,
                        "SEC EDGAR company_tickers_mf.json parse failed"
                    );
                    Ok(None)
                }
                Ok(map) => {
                    self.breaker.lock().await.record_success();
                    let entry = map.get(&ticker_upper).cloned();
                    *self.cik_mf_cache.write().await = Some(map);
                    Ok(entry)
                }
            },
        }
    }

    /// Resolve a fund ticker to a zero-padded 10-digit CIK string.
    ///
    /// Fail-soft. Lookup order:
    ///
    /// 1. `company_tickers.json` (operating companies) via [`Self::lookup_cik`]
    /// 2. `company_tickers_mf.json` (ETF / mutual fund series) via
    ///    [`Self::lookup_cik_mf`]
    ///
    /// ETFs like SPY/QQQ/SOXX live exclusively in (2) — they don't file
    /// 10-K/10-Q so they're not in (1). The fallback ensures the N-PORT-P
    /// holdings fetch finds a CIK for them.
    pub async fn resolve_fund_cik(&self, ticker: &str) -> Option<String> {
        if let Ok(Some(cik)) = self.lookup_cik(ticker).await {
            return Some(format!("{cik:010}"));
        }
        if let Ok(Some(entry)) = self.lookup_cik_mf(ticker).await {
            return Some(format!("{cik:010}", cik = entry.cik));
        }
        None
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

    /// Fetch the most recent N-PORT-P filing for a specific fund **series**
    /// within a trust.
    ///
    /// For multi-series trusts (iShares Trust holds ~100 ETFs under CIK
    /// 1100663), the per-CIK submissions endpoint returns N-PORT-P filings
    /// for every series in the trust undifferentiated. SEC's legacy
    /// `browse-edgar` endpoint accepts a series identifier where a CIK
    /// normally goes and returns an Atom feed listing only that series'
    /// filings — that's the disambiguation path. (EDGAR's full-text search
    /// at `efts.sec.gov` silently ignores the `series=` parameter and falls
    /// back to all matching forms, which gives back the wrong filing.)
    ///
    /// `owner_cik` is the trust CIK that owns the series; it's used to
    /// construct the standard `primary_doc.xml` URL after the accession
    /// number is extracted from the Atom feed.
    ///
    /// Fail-soft contract identical to [`Self::fetch_latest_nport_p`]: any
    /// transport error, non-200 status, parse failure, or empty result set
    /// returns `None` with a `tracing::warn!` log.
    pub async fn fetch_latest_nport_p_for_series(
        &self,
        owner_cik: u32,
        series_id: &str,
        max_age_days: u32,
    ) -> Option<NPortHoldings> {
        {
            let breaker = self.breaker.lock().await;
            if breaker.is_open() {
                tracing::debug!(
                    series_id,
                    "SEC EDGAR circuit breaker open; skipping series N-PORT fetch"
                );
                return None;
            }
        }

        let today = chrono::Utc::now().date_naive();
        let earliest = today - chrono::Duration::days(max_age_days as i64);
        // browse-edgar uses the series ID itself as the CIK parameter. SEC
        // has supported this since the Investment Company Series and Class
        // identifiers were introduced.
        let url = format!(
            "{EDGAR_WWW_BASE_URL}{EDGAR_BROWSE_PATH}\
             ?action=getcompany&CIK={series_id}&type=NPORT-P\
             &dateb=&owner=include&count=10&output=atom",
        );

        self.limiter.acquire().await;
        let (status, body) = match self.http.get(&url).await {
            Ok(pair) => pair,
            Err(transport_err) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_browse_series",
                    series_id,
                    error = %transport_err,
                    "EDGAR browse-edgar transport error"
                );
                return None;
            }
        };
        if status != 200 {
            self.breaker.lock().await.record_failure();
            tracing::warn!(
                kind = "catalyst_fetch_failed",
                source = "sec_edgar_browse_series",
                series_id,
                http_status = status,
                "EDGAR browse-edgar returned non-200"
            );
            return None;
        }

        let filings = match parse_browse_edgar_atom(
            &body,
            owner_cik,
            "NPORT-P",
            &earliest.format("%Y-%m-%d").to_string(),
            &today.format("%Y-%m-%d").to_string(),
        ) {
            Ok(rows) => rows,
            Err(parse_err) => {
                self.breaker.lock().await.record_failure();
                tracing::warn!(
                    kind = "catalyst_fetch_failed",
                    source = "sec_edgar_browse_series",
                    series_id,
                    error = %parse_err,
                    "EDGAR browse-edgar parse failed"
                );
                return None;
            }
        };
        self.breaker.lock().await.record_success();

        let latest = filings.first()?;
        if !is_allowed_sec_document_url(&latest.primary_doc_url) {
            tracing::warn!(
                url = %latest.primary_doc_url,
                series_id,
                "skipping non-SEC or non-HTTPS N-PORT document url"
            );
            return None;
        }
        let filing_date =
            chrono::NaiveDate::parse_from_str(&latest.filing_date, "%Y-%m-%d").ok()?;
        let xml = self.fetch_document_text(&latest.primary_doc_url).await?;
        nport::parse_nport_p(&xml, filing_date)
    }

    /// Fetch the most recent N-PORT-P for a ticker, choosing the right
    /// resolution strategy based on which SEC index the ticker lives in.
    ///
    /// Lookup order:
    /// 1. `company_tickers.json` (operating companies) — uses CIK-level
    ///    `fetch_latest_nport_p`. This path covers the rare case of a fund
    ///    that also files 10-K, and any single-series trust whose CIK alone
    ///    is sufficient.
    /// 2. `company_tickers_mf.json` (ETF / mutual fund series) — uses
    ///    `fetch_latest_nport_p_for_series` with the `(cik, series_id)`
    ///    pair from the MF index. This is the path that disambiguates
    ///    multi-series trusts (iShares, Vanguard, SPDR families).
    ///
    /// Fail-soft: returns `None` when both resolution paths miss or when
    /// the downstream fetch fails.
    pub async fn fetch_latest_nport_p_for_ticker(
        &self,
        ticker: &str,
        max_age_days: u32,
    ) -> Option<NPortHoldings> {
        if let Ok(Some(cik)) = self.lookup_cik(ticker).await {
            let cik_str = format!("{cik:010}");
            if let Some(holdings) = self.fetch_latest_nport_p(&cik_str, max_age_days).await {
                return Some(holdings);
            }
        }
        if let Ok(Some(entry)) = self.lookup_cik_mf(ticker).await {
            return self
                .fetch_latest_nport_p_for_series(entry.cik, &entry.series_id, max_age_days)
                .await;
        }
        None
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

    // ── company_tickers_mf.json parsing ──────────────────────────────────────

    #[test]
    fn parse_company_tickers_mf_extracts_etf_to_entry_map() {
        let json = r#"{
            "fields": ["cik", "seriesId", "classId", "symbol"],
            "data": [
                [1100663, "S000004354", "C000012084", "SOXX"],
                [884394,  "S000003474", "C000009779", "SPY"]
            ]
        }"#;
        let map = parse_company_tickers_mf(json).expect("parse should succeed");
        assert_eq!(
            map.get("SOXX"),
            Some(&MfTickerEntry {
                cik: 1100663,
                series_id: "S000004354".to_owned(),
                class_id: "C000012084".to_owned(),
            })
        );
        assert_eq!(
            map.get("SPY"),
            Some(&MfTickerEntry {
                cik: 884394,
                series_id: "S000003474".to_owned(),
                class_id: "C000009779".to_owned(),
            })
        );
    }

    #[test]
    fn parse_company_tickers_mf_normalizes_ticker_to_uppercase() {
        let json = r#"{
            "fields": ["cik", "seriesId", "classId", "symbol"],
            "data": [[1100663, "S000004354", "C000012084", "soxx"]]
        }"#;
        let map = parse_company_tickers_mf(json).expect("parse should succeed");
        let entry = map.get("SOXX").expect("uppercase key present");
        assert_eq!(entry.cik, 1100663);
        assert_eq!(entry.series_id, "S000004354");
        assert_eq!(map.get("soxx"), None);
    }

    #[test]
    fn parse_company_tickers_mf_rejects_when_series_id_column_moved() {
        // fields[1] is no longer `seriesId`. Tuple types still deserialize
        // (still u32/str/str/str positions) but the header check must catch
        // it — otherwise we'd silently map classId strings to series_id.
        let json = r#"{
            "fields": ["cik", "classId", "seriesId", "symbol"],
            "data": [[1100663, "C000012084", "S000004354", "SOXX"]]
        }"#;
        let err = parse_company_tickers_mf(json).expect_err("schema check must fire");
        assert!(err.contains("schema"), "unexpected error message: {err}");
    }

    #[test]
    fn parse_company_tickers_mf_rejects_unexpected_schema() {
        // fields[3] is `classId` instead of `symbol`. Tuple types still
        // deserialize successfully (all positions still u32/str/str/str),
        // but the `fields` header check must reject — otherwise we'd
        // silently map ClassID strings to CIKs.
        let json = r#"{
            "fields": ["cik", "seriesId", "symbol", "classId"],
            "data": [[1100663, "S000004354", "SOXX", "C000012084"]]
        }"#;
        let err = parse_company_tickers_mf(json).expect_err("schema check must fire");
        assert!(err.contains("schema"), "unexpected error message: {err}");
    }

    #[test]
    fn parse_company_tickers_mf_rejects_when_cik_column_moved() {
        // Same defensive case: cik no longer at position 0.
        let json = r#"{
            "fields": ["seriesId", "cik", "classId", "symbol"],
            "data": [["S000004354", 1100663, "C000012084", "SOXX"]]
        }"#;
        // Serde fails first here (str can't deserialize as u32 at position 0).
        // Either error source is acceptable — both prevent silent corruption.
        assert!(parse_company_tickers_mf(json).is_err());
    }

    #[test]
    fn parse_company_tickers_mf_rejects_malformed_json() {
        assert!(parse_company_tickers_mf("not json").is_err());
        assert!(parse_company_tickers_mf("").is_err());
    }

    // ── resolve_fund_cik MF fallback ─────────────────────────────────────────

    #[tokio::test]
    async fn resolve_fund_cik_falls_back_to_mf_when_equity_map_misses() {
        // First call (equity map) misses; second call (MF map) hits SOXX.
        let mut mock = MockEdgarHttp::new();
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{"0":{"cik_str":320193,"ticker":"AAPL","title":"Apple Inc."}}"#.to_owned(),
                ))
            });
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_MF_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{
                        "fields": ["cik", "seriesId", "classId", "symbol"],
                        "data": [[1100663, "S000004354", "C000012084", "SOXX"]]
                    }"#
                    .to_owned(),
                ))
            });

        let client = SecEdgarClient::with_http(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
        );
        let cik = client.resolve_fund_cik("SOXX").await;
        assert_eq!(cik, Some("0001100663".to_owned()));
    }

    #[tokio::test]
    async fn resolve_fund_cik_prefers_equity_map_when_both_have_ticker() {
        // Some symbols (e.g. ETFs that also operate as companies) could
        // theoretically appear in both. The equity map wins to preserve
        // existing behavior for non-ETF tickers.
        let mut mock = MockEdgarHttp::new();
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{"0":{"cik_str":42,"ticker":"FOO","title":"Foo"}}"#.to_owned(),
                ))
            });
        // No expectation for the MF endpoint — it should never be queried.

        let client = SecEdgarClient::with_http(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
        );
        let cik = client.resolve_fund_cik("FOO").await;
        assert_eq!(cik, Some("0000000042".to_owned()));
    }

    #[tokio::test]
    async fn resolve_fund_cik_returns_none_when_both_maps_miss() {
        let mut mock = MockEdgarHttp::new();
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_PATH))
            .returning(|_| Ok((200, "{}".to_owned())));
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_MF_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{"fields":["cik","seriesId","classId","symbol"],"data":[]}"#.to_owned(),
                ))
            });

        let client = SecEdgarClient::with_http(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
        );
        let cik = client.resolve_fund_cik("UNKNOWN").await;
        assert_eq!(cik, None);
    }

    #[tokio::test]
    async fn lookup_cik_mf_returns_from_cache_without_hitting_http() {
        // Preloaded MF cache → no HTTP call should be made.
        let mock = MockEdgarHttp::new();
        let mut preload = HashMap::new();
        preload.insert(
            "SOXX".to_owned(),
            MfTickerEntry {
                cik: 1100663,
                series_id: "S000004354".to_owned(),
                class_id: "C000012084".to_owned(),
            },
        );
        let client = SecEdgarClient::with_preloaded_mf_cache(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
            preload,
        );
        let entry = client
            .lookup_cik_mf("SOXX")
            .await
            .expect("ok")
            .expect("hit");
        assert_eq!(entry.cik, 1100663);
        assert_eq!(entry.series_id, "S000004354");
    }

    // ── browse-edgar atom feed parsing (series-aware N-PORT) ─────────────────

    fn atom_entry(accession: &str, form: &str, date: &str) -> String {
        format!(
            r#"<entry>
                <category label="form type" scheme="https://www.sec.gov/" term="{form}"/>
                <content type="text/xml">
                    <accession-number>{accession}</accession-number>
                    <act>40</act>
                    <filing-date>{date}</filing-date>
                    <filing-type>{form}</filing-type>
                </content>
                <id>urn:tag:sec.gov,2008:accession-number={accession}</id>
                <title>{form} - iShares Trust</title>
                <updated>{date}T16:01:23-04:00</updated>
            </entry>"#
        )
    }

    fn atom_feed(entries: &[String]) -> String {
        let body = entries.join("\n");
        format!(
            r#"<?xml version="1.0" encoding="ISO-8859-1" ?>
<feed xmlns="http://www.w3.org/2005/Atom">
    <author><email>webmaster@sec.gov</email><name>Webmaster</name></author>
    <company-info>
        <cik>0001100663</cik>
        <conformed-name>iShares Trust</conformed-name>
    </company-info>
    {body}
</feed>"#
        )
    }

    #[test]
    fn parse_browse_edgar_atom_extracts_entries_most_recent_first() {
        let feed = atom_feed(&[
            atom_entry("0001752724-26-001234", "NPORT-P", "2026-04-15"),
            atom_entry("0001752724-26-000567", "NPORT-P", "2026-01-15"),
        ]);
        let rows = parse_browse_edgar_atom(&feed, 1100663, "NPORT-P", "2025-01-01", "2027-01-01")
            .expect("parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].filing_date, "2026-04-15");
        assert_eq!(rows[0].cik, 1100663);
        assert!(
            rows[0]
                .primary_doc_url
                .ends_with("/000175272426001234/primary_doc.xml"),
            "unexpected URL shape: {}",
            rows[0].primary_doc_url
        );
    }

    #[test]
    fn parse_browse_edgar_atom_filters_out_non_matching_forms() {
        let feed = atom_feed(&[atom_entry("0001752724-26-001234", "N-CSR", "2026-04-15")]);
        let rows = parse_browse_edgar_atom(&feed, 1100663, "NPORT-P", "2025-01-01", "2027-01-01")
            .expect("parse");
        assert!(rows.is_empty(), "non-matching form must be filtered out");
    }

    #[test]
    fn parse_browse_edgar_atom_filters_out_filings_outside_date_window() {
        let feed = atom_feed(&[atom_entry("0001752724-24-001234", "NPORT-P", "2024-04-15")]);
        let rows = parse_browse_edgar_atom(&feed, 1100663, "NPORT-P", "2026-01-01", "2026-12-31")
            .expect("parse");
        assert!(rows.is_empty(), "stale filing must be filtered out");
    }

    #[test]
    fn parse_browse_edgar_atom_empty_feed_yields_empty_vec() {
        let feed = atom_feed(&[]);
        let rows = parse_browse_edgar_atom(&feed, 1100663, "NPORT-P", "2025-01-01", "2027-01-01")
            .expect("parse");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_browse_edgar_atom_handles_truncated_xml_as_empty_result() {
        // quick-xml's default config silently reaches EOF on truncated
        // input rather than returning Err. That's acceptable for the
        // fail-soft contract: if SEC returns garbage, we emit no filings
        // rather than panicking. The caller's downstream
        // `filings.first()?` then returns `None`.
        let rows = parse_browse_edgar_atom(
            "<feed><entry><accession-number>missing close",
            1100663,
            "NPORT-P",
            "2025-01-01",
            "2027-01-01",
        )
        .expect("truncated input must not panic");
        assert!(rows.is_empty(), "truncated input must yield no filings");
    }

    #[test]
    fn parse_browse_edgar_atom_rejects_mismatched_closing_tag() {
        // quick-xml *does* error when an explicit close tag mismatches an
        // open tag — that's a real structural error.
        let err = parse_browse_edgar_atom(
            "<feed><entry></wrongclose></entry></feed>",
            1100663,
            "NPORT-P",
            "2025-01-01",
            "2027-01-01",
        );
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn fetch_latest_nport_p_for_ticker_uses_mf_series_path_when_equity_misses() {
        // Equity map misses → MF map returns SOXX with series ID →
        // browse-edgar atom feed returns a recent N-PORT-P for that
        // specific series → primary document fetches → parser returns the
        // holdings.
        let mut mock = MockEdgarHttp::new();
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_PATH))
            .returning(|_| Ok((200, "{}".to_owned())));
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_MF_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{
                        "fields": ["cik", "seriesId", "classId", "symbol"],
                        "data": [[1100663, "S000004354", "C000012084", "SOXX"]]
                    }"#
                    .to_owned(),
                ))
            });
        // browse-edgar request must (a) hit cgi-bin/browse-edgar, (b) pass
        // the series ID as the CIK parameter, and (c) ask for atom output.
        // The series ID is what SEC uses to isolate the specific ETF inside
        // the multi-series trust.
        mock.expect_get()
            .withf(|url: &str| {
                url.contains("/cgi-bin/browse-edgar")
                    && url.contains("CIK=S000004354")
                    && url.contains("output=atom")
            })
            .returning(|_| {
                let feed = r#"<?xml version="1.0" encoding="ISO-8859-1" ?>
<feed xmlns="http://www.w3.org/2005/Atom">
    <author><email>webmaster@sec.gov</email><name>Webmaster</name></author>
    <company-info>
        <cik>0001100663</cik>
        <conformed-name>iShares Trust</conformed-name>
    </company-info>
    <entry>
        <category label="form type" scheme="https://www.sec.gov/" term="NPORT-P"/>
        <content type="text/xml">
            <accession-number>0001752724-26-001234</accession-number>
            <act>40</act>
            <filing-date>2026-04-15</filing-date>
            <filing-type>NPORT-P</filing-type>
        </content>
        <id>urn:tag:sec.gov,2008:accession-number=0001752724-26-001234</id>
        <title>NPORT-P - iShares Trust</title>
        <updated>2026-04-15T16:01:23-04:00</updated>
    </entry>
</feed>"#;
                Ok((200, feed.to_owned()))
            });
        // The primary doc URL — reuse the existing SPY N-PORT fixture so the
        // parser actually returns Some(_) with real holdings shape.
        mock.expect_get()
            .withf(|url: &str| {
                url.contains("/Archives/edgar/data/1100663/000175272426001234/primary_doc.xml")
            })
            .returning(|_| {
                Ok((
                    200,
                    include_str!("../../../tests/fixtures/nport/spy_2026_04_30_excerpt.xml")
                        .to_owned(),
                ))
            });

        let client = SecEdgarClient::with_http(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
        );
        let holdings = client
            .fetch_latest_nport_p_for_ticker("SOXX", 180)
            .await
            .expect("series-aware path must populate holdings");
        assert!(!holdings.holdings.is_empty(), "fixture has holdings rows");
        // Filing date plumbed through the EFTS path:
        assert_eq!(
            holdings.filing_date,
            chrono::NaiveDate::from_ymd_opt(2026, 4, 15).unwrap()
        );
    }

    #[tokio::test]
    async fn fetch_latest_nport_p_for_ticker_returns_none_when_both_indexes_miss() {
        let mut mock = MockEdgarHttp::new();
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_PATH))
            .returning(|_| Ok((200, "{}".to_owned())));
        mock.expect_get()
            .withf(|url: &str| url.ends_with(COMPANY_TICKERS_MF_PATH))
            .returning(|_| {
                Ok((
                    200,
                    r#"{"fields":["cik","seriesId","classId","symbol"],"data":[]}"#.to_owned(),
                ))
            });

        let client = SecEdgarClient::with_http(
            Arc::new(mock),
            SharedRateLimiter::new("test_sec_edgar", 100),
        );
        assert!(
            client
                .fetch_latest_nport_p_for_ticker("UNKNOWN_TICKER", 180)
                .await
                .is_none()
        );
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

    // ── Live tests (require internet, not run in CI) ──────────────────────────

    #[tokio::test]
    #[ignore = "requires live SEC EDGAR connection — run manually"]
    async fn live_lookup_cik_aapl_returns_known_cik() {
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5));
        let cik = client.lookup_cik("AAPL").await.expect("lookup");
        assert_eq!(cik, Some(320193));
    }

    #[tokio::test]
    #[ignore = "requires live SEC EDGAR connection — run manually"]
    async fn live_fetch_recent_8k_filings_for_aapl_returns_nonempty() {
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5));
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
        let client = SecEdgarClient::new(SharedRateLimiter::new("edgar-live", 5));
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
