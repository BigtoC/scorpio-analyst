//! Alpha Vantage earnings-call transcript provider.
//!
//! Implements [`TranscriptProvider`] for Alpha Vantage's
//! `EARNINGS_CALL_TRANSCRIPT` API. Single-key by design; persistent quota
//! and cooldown are deferred (see `TODO(transcripts-quota)` /
//! `TODO(transcripts-cooldown)`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use chrono::NaiveDate;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::config::ApiConfig;
use crate::data::adapters::transcripts::{
    TranscriptEvidence, TranscriptFetch, TranscriptProvider, TranscriptSegment,
};
use crate::data::symbol::validate_symbol;
use crate::data::transcript_cache::TranscriptCacheStore;
use crate::error::TradingError;
use crate::rate_limit::SharedRateLimiter;
use crate::state::{HoldingWeight, SectorWeight};

const BASE_URL: &str = "https://www.alphavantage.co/query";

/// Max length (chars) of provider-returned `Error Message` content embedded in
/// `TradingError`. AV returns short messages in practice; cap to keep error
/// records bounded even if the upstream shape changes.
const MAX_PROVIDER_ERROR_LEN: usize = 200;

/// Alpha Vantage API client for earnings-call transcripts.
///
/// Single-key. Tracks aggregate-health counters so an operator can detect
/// the difference between "this quarter is genuinely unpublished" and "AV
/// integration has been silently broken for N runs."
pub struct AlphaVantageClient {
    key: SecretString,
    rate_limiter: SharedRateLimiter,
    http: reqwest::Client,
    base_url: String,
    cache: Option<TranscriptCacheStore>,
    cache_failure_count: AtomicU64,
    found_count: AtomicU64,
    not_published_count: AtomicU64,
    throttled_count: AtomicU64,
    unavailable_count: AtomicU64,
    schema_error_count: AtomicU64,
    auth_failure_count: AtomicU64,
    auth_failure_logged: AtomicBool,
}

impl std::fmt::Debug for AlphaVantageClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlphaVantageClient")
            .field("rate_limiter", &self.rate_limiter.label())
            .field("found", &self.found_count.load(Ordering::Relaxed))
            .field(
                "not_published",
                &self.not_published_count.load(Ordering::Relaxed),
            )
            .field("throttled", &self.throttled_count.load(Ordering::Relaxed))
            .field(
                "unavailable",
                &self.unavailable_count.load(Ordering::Relaxed),
            )
            .field(
                "schema_errors",
                &self.schema_error_count.load(Ordering::Relaxed),
            )
            .field(
                "auth_failures",
                &self.auth_failure_count.load(Ordering::Relaxed),
            )
            .field(
                "cache_failure_count",
                &self.cache_failure_count.load(Ordering::Relaxed),
            )
            .finish()
    }
}

/// Internal serde struct for Alpha Vantage transcript API responses.
#[derive(Deserialize)]
struct AlphaVantageTranscriptResponse {
    symbol: Option<String>,
    quarter: Option<String>,
    transcript: Option<Vec<TranscriptSegment>>,
    /// Rate-limit / daily-quota signal.
    #[serde(rename = "Note")]
    note: Option<String>,
    /// Catch-all informational field. Alpha Vantage uses this for rate-limit,
    /// premium-required, and promotional messages — the body text is parsed
    /// to route into the right `TranscriptFetch` variant.
    #[serde(rename = "Information")]
    information: Option<String>,
    /// Per-request hard error (bad symbol, malformed params).
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
}

/// Outcome of an Alpha Vantage `ETF_PROFILE` fetch/parse. Mirrors the
/// transcript provider's fail-soft taxonomy: a present profile, a transient
/// rate-limit, an endpoint that is gated/unavailable, or no profile data.
#[derive(Debug, Clone, PartialEq)]
pub enum EtfProfileFetch {
    Found(EtfProfileData),
    Throttled,
    Unavailable,
    NotAvailable,
}

/// Parsed Alpha Vantage `ETF_PROFILE` payload. Weights are expressed as
/// percentages (e.g. `8.4` for an 0.084 decimal weight); ratios/yields stay in
/// their upstream decimal form.
#[derive(Debug, Clone, PartialEq)]
pub struct EtfProfileData {
    pub holdings: Vec<HoldingWeight>,
    pub sectors: Vec<SectorWeight>,
    pub aum_usd: Option<f64>,
    pub expense_ratio_pct: Option<f64>,
    pub portfolio_turnover_pct: Option<f64>,
    pub distribution_yield_pct: Option<f64>,
    pub inception_date: Option<NaiveDate>,
    pub leverage_factor: Option<f64>,
}

#[derive(Deserialize)]
struct AlphaVantageEtfProfileResponse {
    net_assets: Option<String>,
    net_expense_ratio: Option<String>,
    portfolio_turnover: Option<String>,
    dividend_yield: Option<String>,
    inception_date: Option<String>,
    leveraged: Option<String>,
    #[serde(default)]
    sectors: Vec<AlphaVantageSectorRow>,
    #[serde(default)]
    holdings: Vec<AlphaVantageHoldingRow>,
    #[serde(rename = "Note")]
    note: Option<String>,
    #[serde(rename = "Information")]
    information: Option<String>,
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct AlphaVantageSectorRow {
    sector: Option<String>,
    weight: Option<String>,
}

#[derive(Deserialize)]
struct AlphaVantageHoldingRow {
    symbol: Option<String>,
    description: Option<String>,
    weight: Option<String>,
}

impl AlphaVantageClient {
    /// Construct a new client. Returns `Err(TradingError::Config)` if no key is configured.
    ///
    /// **Security:** The error path uses a static literal — no key material is interpolated.
    pub fn new(
        api: &ApiConfig,
        limiter: SharedRateLimiter,
        cache: Option<TranscriptCacheStore>,
    ) -> Result<Self, TradingError> {
        let key = api
            .alpha_vantage_api_key
            .as_ref()
            .ok_or_else(|| {
                TradingError::Config(anyhow::anyhow!("SCORPIO_ALPHA_VANTAGE_API_KEY is not set"))
            })?
            .clone();

        Ok(Self {
            key,
            rate_limiter: limiter,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| TradingError::Config(anyhow::anyhow!("reqwest client build: {e}")))?,
            base_url: BASE_URL.to_owned(),
            cache,
            cache_failure_count: AtomicU64::new(0),
            found_count: AtomicU64::new(0),
            not_published_count: AtomicU64::new(0),
            throttled_count: AtomicU64::new(0),
            unavailable_count: AtomicU64::new(0),
            schema_error_count: AtomicU64::new(0),
            auth_failure_count: AtomicU64::new(0),
            auth_failure_logged: AtomicBool::new(false),
        })
    }

    /// Test-only constructor with a dummy key and a non-routable base URL.
    ///
    /// **Hermetic-by-default:** `base_url` is set to `http://127.0.0.1:1` so a
    /// test that accidentally hits the network fails fast (connection refused)
    /// rather than reaching live Alpha Vantage.
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            "http://127.0.0.1:1/query".to_owned(),
            None,
        )
    }

    fn new_with_base_url(
        key: SecretString,
        limiter: SharedRateLimiter,
        base_url: String,
        cache: Option<TranscriptCacheStore>,
    ) -> Self {
        Self {
            key,
            rate_limiter: limiter,
            http: reqwest::Client::new(),
            base_url,
            cache,
            cache_failure_count: AtomicU64::new(0),
            found_count: AtomicU64::new(0),
            not_published_count: AtomicU64::new(0),
            throttled_count: AtomicU64::new(0),
            unavailable_count: AtomicU64::new(0),
            schema_error_count: AtomicU64::new(0),
            auth_failure_count: AtomicU64::new(0),
            auth_failure_logged: AtomicBool::new(false),
        }
    }

    /// Return the number of cache write failures observed.
    pub fn cache_failure_count(&self) -> u64 {
        self.cache_failure_count.load(Ordering::Relaxed)
    }

    /// Validate the quarter format (`"YYYYQN"` where N is 1-4) using byte
    /// arithmetic — no `regex` crate dependency for a 6-char structural check.
    fn validate_quarter(quarter: &str) -> Result<(), TradingError> {
        let b = quarter.as_bytes();
        let ok = b.len() == 6
            && b[0..4].iter().all(|c| c.is_ascii_digit())
            && b[4] == b'Q'
            && matches!(b[5], b'1'..=b'4');
        if !ok {
            return Err(TradingError::SchemaViolation {
                message: format!("invalid quarter format (expected YYYYQN, N=1..4): {quarter:?}"),
            });
        }
        Ok(())
    }

    /// Truncate provider-returned diagnostics before embedding in an internal
    /// error/logging value. Third-party content should not be unbounded.
    ///
    /// Counts by chars (not bytes) to avoid panicking on multi-byte UTF-8
    /// boundaries.
    fn truncate_provider_msg(msg: &str) -> String {
        let sanitized: String = msg
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let collapsed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.chars().count() <= MAX_PROVIDER_ERROR_LEN {
            collapsed
        } else {
            let mut s: String = collapsed.chars().take(MAX_PROVIDER_ERROR_LEN).collect();
            s.push('…');
            s
        }
    }

    /// Classify an `Information` / `Note` body into a transcript fetch outcome.
    fn classify_information(msg: &str) -> TranscriptFetch {
        let lower = msg.to_ascii_lowercase();
        let truncated = Self::truncate_provider_msg(msg);
        if lower.contains("call frequency")
            || lower.contains("per minute")
            || lower.contains("requests per")
            || lower.contains("daily limit")
            || lower.contains("daily quota")
            || lower.contains("exceeded")
        {
            debug!(
                provider = "alpha_vantage",
                body = %truncated,
                "classified Information/Note as Throttled"
            );
            return TranscriptFetch::Throttled;
        }
        if lower.contains("premium") || lower.contains("standard plan") {
            debug!(
                provider = "alpha_vantage",
                body = %truncated,
                "classified Information/Note as Unavailable (premium endpoint)"
            );
            return TranscriptFetch::Unavailable;
        }
        debug!(
            provider = "alpha_vantage",
            body = %truncated,
            "classified Information/Note as NotPublished (unrecognised body)"
        );
        TranscriptFetch::NotPublished
    }

    fn build_url(&self) -> String {
        format!("{}?function=EARNINGS_CALL_TRANSCRIPT", self.base_url)
    }

    /// Parse the raw JSON response.
    fn parse_response(raw: &str) -> Result<TranscriptFetch, TradingError> {
        debug!(
            provider = "alpha_vantage",
            body_bytes = raw.len(),
            "parsing Alpha Vantage response body"
        );

        let resp: AlphaVantageTranscriptResponse = serde_json::from_str(raw).map_err(|e| {
            warn!(
                provider = "alpha_vantage",
                error = %e,
                body_bytes = raw.len(),
                "response deserialization failed"
            );
            TradingError::SchemaViolation {
                message: format!("Alpha Vantage response deserialization failed: {e}"),
            }
        })?;

        if let Some(msg) = &resp.error_message {
            warn!(
                provider = "alpha_vantage",
                error = %Self::truncate_provider_msg(msg),
                "response contained Error Message field"
            );
            return Err(TradingError::SchemaViolation {
                message: format!("Alpha Vantage error: {}", Self::truncate_provider_msg(msg)),
            });
        }

        if let Some(body) = resp.note.as_deref().or(resp.information.as_deref()) {
            return Ok(Self::classify_information(body));
        }

        match resp.transcript {
            Some(segments) if !segments.is_empty() => {
                let symbol = resp.symbol.unwrap_or_default().to_uppercase();
                let call_date = resp.quarter.unwrap_or_default();
                info!(
                    provider = "alpha_vantage",
                    symbol = %symbol,
                    quarter = %call_date,
                    segments = segments.len(),
                    "parsed transcript response: Found"
                );
                Ok(TranscriptFetch::Found(TranscriptEvidence {
                    symbol,
                    call_date,
                    segments,
                }))
            }
            Some(_) => {
                debug!(
                    provider = "alpha_vantage",
                    symbol = ?resp.symbol,
                    quarter = ?resp.quarter,
                    "transcript field present but empty -> NotPublished"
                );
                Ok(TranscriptFetch::NotPublished)
            }
            None => {
                debug!(
                    provider = "alpha_vantage",
                    symbol = ?resp.symbol,
                    quarter = ?resp.quarter,
                    "transcript field absent -> NotPublished"
                );
                Ok(TranscriptFetch::NotPublished)
            }
        }
    }

    fn record_outcome(&self, outcome: &TranscriptFetch) {
        let counter = match outcome {
            TranscriptFetch::Found(_) => &self.found_count,
            TranscriptFetch::NotPublished => &self.not_published_count,
            TranscriptFetch::Throttled => &self.throttled_count,
            TranscriptFetch::Unavailable => &self.unavailable_count,
        };
        let new_count = counter.fetch_add(1, Ordering::Relaxed) + 1;
        debug!(
            provider = "alpha_vantage",
            outcome = ?outcome,
            counter = new_count,
            found_total = self.found_count.load(Ordering::Relaxed),
            not_published_total = self.not_published_count.load(Ordering::Relaxed),
            throttled_total = self.throttled_count.load(Ordering::Relaxed),
            unavailable_total = self.unavailable_count.load(Ordering::Relaxed),
            "recorded transcript fetch outcome"
        );
    }

    fn record_schema_error(&self) {
        self.schema_error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Escalate an authentication/authorization failure (HTTP 401/403).
    /// Increments the auth-failure counter and emits a single `error!`-level
    /// log on the first occurrence of the process lifetime.
    fn escalate_auth_failure(&self, status: reqwest::StatusCode) {
        self.auth_failure_count.fetch_add(1, Ordering::Relaxed);
        let already_logged = self.auth_failure_logged.swap(true, Ordering::Relaxed);
        if !already_logged {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                tracing::error!(
                    provider = "alpha_vantage",
                    %status,
                    "Alpha Vantage rejected the API key (401) — rotate \
                     SCORPIO_ALPHA_VANTAGE_API_KEY. Transcripts will fail-open \
                     to degraded mode until the key is corrected."
                );
            } else {
                tracing::error!(
                    provider = "alpha_vantage",
                    %status,
                    "Alpha Vantage refused the request (403) — likely a plan, \
                     region, or edge-policy restriction; verify the account \
                     tier and source IP. The key itself may be valid. \
                     Transcripts will fail-open to degraded mode."
                );
            }
        }
    }

    fn parse_optional_f64(raw: Option<&str>) -> Option<f64> {
        let value = raw?.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("n/a") {
            return None;
        }
        value.replace(',', "").parse::<f64>().ok()
    }

    fn parse_optional_date(raw: Option<&str>) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(raw?.trim(), "%Y-%m-%d").ok()
    }

    /// Upstream weights are decimal fractions (`0.084`); render as percent.
    fn parse_decimal_weight_pct(raw: Option<&str>) -> Option<f64> {
        Self::parse_optional_f64(raw).map(|value| value * 100.0)
    }

    /// `ETF_PROFILE.leveraged` is `"NO"`/`"YES"`. Only plain (`"NO"`) funds get a
    /// known `1.0` factor; a leveraged fund's true factor is not in this payload.
    fn parse_leverage_factor(raw: Option<&str>) -> Option<f64> {
        match raw?.trim().to_ascii_uppercase().as_str() {
            "NO" => Some(1.0),
            _ => None,
        }
    }

    /// Parse a raw `ETF_PROFILE` JSON body into an [`EtfProfileFetch`]. Provider
    /// diagnostics (`Note`/`Information`) are classified fail-soft; an
    /// `Error Message` is a schema violation (the only `Err` path).
    pub(crate) fn parse_etf_profile_response(raw: &str) -> Result<EtfProfileFetch, TradingError> {
        let resp: AlphaVantageEtfProfileResponse =
            serde_json::from_str(raw).map_err(|e| TradingError::SchemaViolation {
                message: format!("Alpha Vantage ETF_PROFILE response deserialization failed: {e}"),
            })?;

        if let Some(msg) = &resp.error_message {
            return Err(TradingError::SchemaViolation {
                message: format!(
                    "Alpha Vantage ETF_PROFILE error: {}",
                    Self::truncate_provider_msg(msg)
                ),
            });
        }

        if let Some(body) = resp.note.as_deref().or(resp.information.as_deref()) {
            // ETF_PROFILE has no "not published this quarter" state; a non-throttle
            // informational body means the endpoint is gated (premium plan,
            // region, etc.). `classify_information`'s premium keyword set is
            // narrower than AV's plan/availability gating language, so refine it.
            return Ok(match Self::classify_information(body) {
                TranscriptFetch::Throttled => EtfProfileFetch::Throttled,
                TranscriptFetch::Unavailable => EtfProfileFetch::Unavailable,
                TranscriptFetch::NotPublished | TranscriptFetch::Found(_) => {
                    let lower = body.to_ascii_lowercase();
                    if lower.contains("plan")
                        || lower.contains("not available")
                        || lower.contains("subscribe")
                    {
                        EtfProfileFetch::Unavailable
                    } else {
                        EtfProfileFetch::NotAvailable
                    }
                }
            });
        }

        let holdings = resp
            .holdings
            .into_iter()
            .filter_map(|row| {
                let weight_pct = Self::parse_decimal_weight_pct(row.weight.as_deref())?;
                let name = row.description.or_else(|| row.symbol.clone())?;
                Some(HoldingWeight {
                    cusip: None,
                    ticker: row.symbol.filter(|s| !s.trim().is_empty()),
                    name,
                    weight_pct,
                    value_usd: None,
                })
            })
            .collect();

        let sectors = resp
            .sectors
            .into_iter()
            .filter_map(|row| {
                Some(SectorWeight {
                    sector: row.sector.filter(|s| !s.trim().is_empty())?,
                    weight_pct: Self::parse_decimal_weight_pct(row.weight.as_deref())?,
                })
            })
            .collect();

        Ok(EtfProfileFetch::Found(EtfProfileData {
            holdings,
            sectors,
            aum_usd: Self::parse_optional_f64(resp.net_assets.as_deref()),
            expense_ratio_pct: Self::parse_optional_f64(resp.net_expense_ratio.as_deref()),
            portfolio_turnover_pct: Self::parse_optional_f64(resp.portfolio_turnover.as_deref()),
            distribution_yield_pct: Self::parse_optional_f64(resp.dividend_yield.as_deref()),
            inception_date: Self::parse_optional_date(resp.inception_date.as_deref()),
            leverage_factor: Self::parse_leverage_factor(resp.leveraged.as_deref()),
        }))
    }

    fn build_etf_profile_url(&self) -> String {
        format!("{}?function=ETF_PROFILE", self.base_url)
    }

    /// Fetch the Alpha Vantage `ETF_PROFILE` for `symbol`, fail-soft: transient
    /// throttles, auth/region gating, server errors, and connect/timeout faults
    /// degrade to a non-`Err` outcome so the ETF pipeline keeps running.
    pub async fn fetch_etf_profile(&self, symbol: &str) -> Result<EtfProfileFetch, TradingError> {
        validate_symbol(symbol)?;
        self.rate_limiter.acquire().await;

        let response = self
            .http
            .get(self.build_etf_profile_url())
            .query(&[("symbol", symbol), ("apikey", self.key.expose_secret())])
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let body = resp.text().await.map_err(|e| {
                    TradingError::Config(anyhow::anyhow!(
                        "Alpha Vantage ETF_PROFILE body read error: {e}"
                    ))
                })?;
                Self::parse_etf_profile_response(&body)
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                Ok(EtfProfileFetch::Throttled)
            }
            Ok(resp)
                if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                    || resp.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                self.escalate_auth_failure(resp.status());
                Ok(EtfProfileFetch::Unavailable)
            }
            Ok(resp) if resp.status().is_server_error() => Ok(EtfProfileFetch::Unavailable),
            Ok(resp) => Err(TradingError::Config(anyhow::anyhow!(
                "Alpha Vantage ETF_PROFILE HTTP error: {}",
                resp.status()
            ))),
            Err(e) if e.is_timeout() || e.is_connect() => Ok(EtfProfileFetch::Unavailable),
            Err(e) => Err(TradingError::Config(anyhow::anyhow!(
                "Alpha Vantage ETF_PROFILE request error: {}",
                e.without_url()
            ))),
        }
    }
}

#[async_trait::async_trait]
impl TranscriptProvider for AlphaVantageClient {
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<TranscriptFetch, TradingError> {
        info!(
            provider = "alpha_vantage",
            symbol,
            quarter = as_of_date,
            "fetch_transcript: invoked"
        );

        validate_symbol(symbol)?;
        Self::validate_quarter(as_of_date)?;

        if let Some(cache) = &self.cache {
            if let Some(cached) = cache.get(symbol, as_of_date).await {
                debug!(symbol, quarter = as_of_date, "transcript cache hit");
                return Ok(cached);
            }
            info!(
                symbol,
                quarter = as_of_date,
                "transcript cache miss, fetching from Alpha Vantage"
            );
        }

        debug!(
            provider = "alpha_vantage",
            limiter = %self.rate_limiter.label(),
            "fetch_transcript: acquiring rate-limit permit"
        );
        self.rate_limiter.acquire().await;

        let request_started = std::time::Instant::now();
        debug!(
            provider = "alpha_vantage",
            url = %self.build_url(),
            symbol,
            quarter = as_of_date,
            "fetch_transcript: sending request"
        );

        let response = self
            .http
            .get(self.build_url())
            .query(&[
                ("symbol", symbol),
                ("quarter", as_of_date),
                ("apikey", self.key.expose_secret()),
            ])
            .send()
            .await;

        let elapsed_ms = request_started.elapsed().as_millis() as u64;

        let outcome = match response {
            Ok(resp) => {
                let status = resp.status();
                info!(
                    provider = "alpha_vantage",
                    symbol,
                    quarter = as_of_date,
                    %status,
                    elapsed_ms,
                    "fetch_transcript: received HTTP response"
                );

                if status.is_success() {
                    let body = resp.text().await.map_err(|e| {
                        warn!(
                            provider = "alpha_vantage",
                            symbol,
                            quarter = as_of_date,
                            error = %e,
                            "fetch_transcript: response body read failed"
                        );
                        TradingError::Config(anyhow::anyhow!("response read error: {e}"))
                    })?;
                    match Self::parse_response(&body) {
                        Ok(o) => o,
                        Err(e) => {
                            self.record_schema_error();
                            warn!(
                                provider = "alpha_vantage",
                                symbol,
                                quarter = as_of_date,
                                error = %e,
                                schema_error_total = self.schema_error_count.load(Ordering::Relaxed),
                                "fetch_transcript: response parse failed (schema error)"
                            );
                            return Err(e);
                        }
                    }
                } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    warn!(
                        provider = "alpha_vantage",
                        symbol,
                        quarter = as_of_date,
                        "fetch_transcript: HTTP 429 -> Throttled"
                    );
                    TranscriptFetch::Throttled
                } else if status == reqwest::StatusCode::UNAUTHORIZED
                    || status == reqwest::StatusCode::FORBIDDEN
                {
                    self.escalate_auth_failure(status);
                    TranscriptFetch::Unavailable
                } else if status.is_server_error() {
                    warn!(
                        provider = "alpha_vantage",
                        symbol,
                        quarter = as_of_date,
                        %status,
                        "fetch_transcript: 5xx response -> Unavailable"
                    );
                    TranscriptFetch::Unavailable
                } else {
                    warn!(
                        provider = "alpha_vantage",
                        symbol,
                        quarter = as_of_date,
                        %status,
                        "fetch_transcript: unexpected HTTP status -> Err"
                    );
                    return Err(TradingError::Config(anyhow::anyhow!(
                        "Alpha Vantage HTTP error: {status}"
                    )));
                }
            }
            Err(e) if e.is_timeout() || e.is_connect() => {
                warn!(
                    provider = "alpha_vantage",
                    symbol,
                    quarter = as_of_date,
                    elapsed_ms,
                    is_timeout = e.is_timeout(),
                    is_connect = e.is_connect(),
                    "fetch_transcript: network failure -> Unavailable (fail-open)"
                );
                TranscriptFetch::Unavailable
            }
            Err(e) => {
                // Scrub the URL (which contains the apikey query param) before
                // embedding the error.
                let scrubbed = e.without_url();
                warn!(
                    provider = "alpha_vantage",
                    symbol,
                    quarter = as_of_date,
                    elapsed_ms,
                    error = %scrubbed,
                    "fetch_transcript: request error -> Err"
                );
                return Err(TradingError::Config(anyhow::anyhow!(
                    "Alpha Vantage request error: {scrubbed}"
                )));
            }
        };

        self.record_outcome(&outcome);

        if let Some(cache) = &self.cache
            && let Err(_err) = cache.put(symbol, as_of_date, &outcome).await
        {
            self.cache_failure_count.fetch_add(1, Ordering::Relaxed);
            warn!(
                symbol,
                quarter = as_of_date,
                error.kind = "storage",
                "transcript cache put failed"
            );
        }

        info!(
            provider = "alpha_vantage",
            symbol,
            quarter = as_of_date,
            outcome = ?outcome,
            elapsed_ms,
            "fetch_transcript: completed"
        );
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Constructor ─────────────────────────────────────────────────────

    #[test]
    fn debug_does_not_leak_secret() {
        let mut api = ApiConfig::default();
        let secret = "AVKEY-DO-NOT-LEAK-123";
        api.alpha_vantage_api_key = Some(SecretString::from(secret));
        let client = AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test"), None)
            .expect("construct");
        let debug = format!("{client:?}");
        assert!(
            !debug.contains(secret),
            "Debug must not expose the secret value"
        );
    }

    #[test]
    fn constructor_missing_key_uses_static_error_message() {
        let api = ApiConfig::default();
        let err =
            AlphaVantageClient::new(&api, SharedRateLimiter::disabled("test"), None).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("SCORPIO_ALPHA_VANTAGE_API_KEY is not set"),
            "expected static literal error, got: {msg}"
        );
        // Ensure no key material can ever appear here (no SecretString interpolation).
        assert!(
            !msg.contains("Some("),
            "must not Debug-format the Option<SecretString>"
        );
    }

    // ── ETF profile parsing ─────────────────────────────────────────────

    #[test]
    fn parse_etf_profile_converts_decimal_weights_and_profile_fields() {
        let raw = include_str!("../../tests/fixtures/alpha_vantage/soxx_etf_profile.json");
        let EtfProfileFetch::Found(profile) =
            AlphaVantageClient::parse_etf_profile_response(raw).expect("parse profile")
        else {
            panic!("expected Found profile");
        };

        assert_eq!(profile.aum_usd, Some(12_300_000_000.0));
        assert_eq!(profile.expense_ratio_pct, Some(0.0035));
        assert_eq!(profile.portfolio_turnover_pct, Some(0.24));
        assert_eq!(profile.distribution_yield_pct, Some(0.0061));
        assert_eq!(
            profile.inception_date,
            Some(chrono::NaiveDate::from_ymd_opt(2001, 7, 10).unwrap())
        );
        assert_eq!(profile.leverage_factor, Some(1.0));
        assert_eq!(profile.holdings[0].ticker.as_deref(), Some("NVDA"));
        assert!((profile.holdings[0].weight_pct - 8.4).abs() < 1e-9);
        assert_eq!(profile.holdings.len(), 2, "n/a holding weight is skipped");
        assert!((profile.sectors[0].weight_pct - 78.2).abs() < 1e-9);
    }

    #[test]
    fn parse_etf_profile_classifies_provider_diagnostics_without_secret_text() {
        // `assert_eq!` on the unwrapped `EtfProfileFetch` (not the `Result`)
        // because `TradingError` is not `PartialEq` (it carries `anyhow::Error`).
        assert_eq!(
            AlphaVantageClient::parse_etf_profile_response(
                r#"{"Note":"Thank you. Standard call frequency is 5 calls per minute."}"#
            )
            .expect("note classifies"),
            EtfProfileFetch::Throttled
        );
        assert_eq!(
            AlphaVantageClient::parse_etf_profile_response(
                r#"{"Information":"This endpoint is not available under your current plan."}"#
            )
            .expect("information classifies"),
            EtfProfileFetch::Unavailable
        );
        let err = AlphaVantageClient::parse_etf_profile_response(
            r#"{"Error Message":"bad api key\nSECRET"}"#,
        )
        .expect_err("error message should be schema violation");
        assert!(!format!("{err}").contains('\n'));
    }

    // ── Input validation ────────────────────────────────────────────────

    #[tokio::test]
    async fn invalid_quarter_format_rejected() {
        let client = AlphaVantageClient::for_test();
        let err = client
            .fetch_transcript("AAPL", "2025-Q1")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("invalid quarter format"));
    }

    #[test]
    fn quarter_validator_accepts_canonical_form() {
        assert!(AlphaVantageClient::validate_quarter("2025Q1").is_ok());
        assert!(AlphaVantageClient::validate_quarter("2025Q4").is_ok());
        assert!(AlphaVantageClient::validate_quarter("2025Q0").is_err());
        assert!(AlphaVantageClient::validate_quarter("2025Q5").is_err());
        assert!(AlphaVantageClient::validate_quarter("25Q1").is_err());
        assert!(AlphaVantageClient::validate_quarter("2025-Q1").is_err());
    }

    #[tokio::test]
    async fn invalid_symbol_rejected() {
        let client = AlphaVantageClient::for_test();
        let err = client.fetch_transcript("", "2025Q1").await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("invalid symbol") || msg.contains("empty"));
    }

    // ── Response parsing ────────────────────────────────────────────────

    #[test]
    fn parse_transcript_response() {
        let json = r#"{
            "symbol": "COIN",
            "quarter": "2024Q1",
            "transcript": [
                { "speaker": "Alesia Haas", "title": "CFO",
                  "content": "Thank you, operator...", "sentiment": 0.85 },
                { "speaker": "Brian Armstrong", "title": "CEO",
                  "content": "Strong quarter.", "sentiment": null }
            ]
        }"#;
        let result = AlphaVantageClient::parse_response(json).expect("parse");
        match result {
            TranscriptFetch::Found(evidence) => {
                assert_eq!(evidence.symbol, "COIN");
                assert_eq!(evidence.call_date, "2024Q1");
                assert_eq!(evidence.segments.len(), 2);
                assert_eq!(evidence.segments[0].sentiment, Some(0.85));
                assert!(evidence.segments[1].sentiment.is_none());
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn information_field_classified_by_content() {
        assert_eq!(
            AlphaVantageClient::parse_response(
                r#"{"Information": "Our standard API call frequency is 5 per minute..."}"#
            )
            .unwrap(),
            TranscriptFetch::Throttled
        );
        assert_eq!(
            AlphaVantageClient::parse_response(
                r#"{"Information": "You have exceeded the daily limit"}"#
            )
            .unwrap(),
            TranscriptFetch::Throttled
        );
        assert_eq!(
            AlphaVantageClient::parse_response(
                r#"{"Information": "This is a premium endpoint. Visit alphavantage.co/premium..."}"#
            )
            .unwrap(),
            TranscriptFetch::Unavailable
        );
        assert_eq!(
            AlphaVantageClient::parse_response(
                r#"{"Information": "Thank you for being a long-time user!"}"#
            )
            .unwrap(),
            TranscriptFetch::NotPublished
        );
    }

    #[test]
    fn note_field_classified_as_rate_limit() {
        assert_eq!(
            AlphaVantageClient::parse_response(
                r#"{"Note": "Thank you. Standard call frequency is 5 calls per minute."}"#
            )
            .unwrap(),
            TranscriptFetch::Throttled
        );
    }

    #[test]
    fn parse_error_message_truncates_long_provider_text() {
        let long_msg = "x".repeat(500);
        let json = format!(r#"{{"Error Message": "{long_msg}"}}"#);
        let err = AlphaVantageClient::parse_response(&json).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.len() < 500, "provider message must be bounded");
        assert!(msg.contains("Alpha Vantage error"));
    }

    #[test]
    fn parse_error_message_strips_control_characters() {
        let json = r#"{"Error Message": "bad key\nnext\u0007line"}"#;
        let err = AlphaVantageClient::parse_response(json).unwrap_err();
        let msg = format!("{err}");
        assert!(
            !msg.chars().any(char::is_control),
            "provider message must be log-safe: {msg:?}"
        );
        assert!(msg.contains("bad key"));
        assert!(msg.contains("next"));
        assert!(msg.contains("line"));
    }

    #[test]
    fn parse_empty_transcript_array() {
        let json = r#"{"symbol": "AAPL", "quarter": "2025Q1", "transcript": []}"#;
        assert_eq!(
            AlphaVantageClient::parse_response(json).unwrap(),
            TranscriptFetch::NotPublished
        );
    }

    #[test]
    fn parse_missing_transcript_field() {
        let json = r#"{"symbol": "AAPL", "quarter": "2025Q1"}"#;
        assert_eq!(
            AlphaVantageClient::parse_response(json).unwrap(),
            TranscriptFetch::NotPublished
        );
    }

    #[test]
    fn parse_sentiment_encoded_as_string() {
        // Alpha Vantage encodes sentiment as a JSON string in live responses
        // (e.g. `"0.0"`, `"0.9"`), even though docs describe it as a number.
        let json = r#"{
            "symbol": "GLW",
            "quarter": "2026Q1",
            "transcript": [
                {"speaker": "Op", "title": "Operator", "content": "Hi.", "sentiment": "0.0"},
                {"speaker": "CEO", "title": "CEO", "content": "Good Q.", "sentiment": "0.9"},
                {"speaker": "CFO", "title": "CFO", "content": "Yes.", "sentiment": ""}
            ]
        }"#;
        if let TranscriptFetch::Found(evidence) = AlphaVantageClient::parse_response(json).unwrap()
        {
            assert_eq!(evidence.segments[0].sentiment, Some(0.0));
            assert_eq!(evidence.segments[1].sentiment, Some(0.9));
            assert!(evidence.segments[2].sentiment.is_none());
        } else {
            panic!("expected Found");
        }
    }

    #[test]
    fn parse_partial_sentiment() {
        let json = r#"{
            "symbol": "AAPL", "quarter": "2025Q1",
            "transcript": [
                {"speaker": "A", "title": "B", "content": "C", "sentiment": 0.5},
                {"speaker": "D", "title": "E", "content": "F"}
            ]
        }"#;
        if let TranscriptFetch::Found(evidence) = AlphaVantageClient::parse_response(json).unwrap()
        {
            assert_eq!(evidence.segments[0].sentiment, Some(0.5));
            assert!(evidence.segments[1].sentiment.is_none());
        } else {
            panic!("expected Found");
        }
    }

    // ── Health counters ─────────────────────────────────────────────────

    #[test]
    fn record_outcome_increments_correct_counter() {
        let client = AlphaVantageClient::for_test();
        client.record_outcome(&TranscriptFetch::NotPublished);
        client.record_outcome(&TranscriptFetch::NotPublished);
        client.record_outcome(&TranscriptFetch::Throttled);
        assert_eq!(client.not_published_count.load(Ordering::Relaxed), 2);
        assert_eq!(client.throttled_count.load(Ordering::Relaxed), 1);
        assert_eq!(client.found_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn record_schema_error_increments_counter() {
        let client = AlphaVantageClient::for_test();
        client.record_schema_error();
        client.record_schema_error();
        assert_eq!(client.schema_error_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn escalate_auth_failure_increments_and_is_idempotent_for_logging() {
        let client = AlphaVantageClient::for_test();
        client.escalate_auth_failure(reqwest::StatusCode::UNAUTHORIZED);
        client.escalate_auth_failure(reqwest::StatusCode::FORBIDDEN);
        assert_eq!(client.auth_failure_count.load(Ordering::Relaxed), 2);
        assert!(client.auth_failure_logged.load(Ordering::Relaxed));
    }

    // ── Cache integration ──────────────────────────────────────────────

    use crate::data::transcript_cache::TranscriptCacheStore;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn sample_found(symbol: &str, quarter: &str) -> TranscriptFetch {
        TranscriptFetch::Found(TranscriptEvidence {
            symbol: symbol.to_owned(),
            call_date: quarter.to_owned(),
            segments: vec![TranscriptSegment {
                speaker: "Tim Cook".to_owned(),
                title: "CEO".to_owned(),
                content: "Hello everyone.".to_owned(),
                sentiment: Some(0.5),
            }],
        })
    }

    fn spawn_transcript_server(
        status_line: &'static str,
        body: &'static str,
        calls: Arc<AtomicUsize>,
        expected_requests: usize,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");

        std::thread::spawn(move || {
            for _ in 0..expected_requests {
                let (mut stream, _) = listener.accept().expect("accept");
                let _ = std::io::Read::read(&mut stream, &mut [0u8; 1024]);
                calls.fetch_add(1, Ordering::SeqCst);

                let response = format!(
                    "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body,
                );
                std::io::Write::write_all(&mut stream, response.as_bytes()).expect("write");
            }
        });

        format!("http://{addr}/query")
    }

    #[tokio::test]
    async fn fetch_transcript_returns_cached_result_before_network() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        let cached = sample_found("AAPL", "2025Q1");

        cache
            .put("AAPL", "2025Q1", &cached)
            .await
            .expect("seed cache");

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            "http://127.0.0.1:1/query".to_owned(),
            Some(cache),
        );

        let result = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("cache hit should succeed");

        assert_eq!(result, cached);
    }

    #[tokio::test]
    async fn fetch_transcript_caches_found_results_after_first_api_call() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        let calls = Arc::new(AtomicUsize::new(0));

        let base_url = spawn_transcript_server(
            "HTTP/1.1 200 OK",
            r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[{"speaker":"Tim Cook","title":"CEO","content":"Hello","sentiment":0.5}]}"#,
            Arc::clone(&calls),
            1,
        );

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            base_url,
            Some(cache),
        );

        let first = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("first call");
        let second = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("second call");

        assert!(matches!(first, TranscriptFetch::Found(_)));
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.cache_failure_count(), 0);
    }

    #[tokio::test]
    async fn fetch_transcript_uses_api_when_cache_is_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        cache.close_for_test().await;
        let calls = Arc::new(AtomicUsize::new(0));

        let base_url = spawn_transcript_server(
            "HTTP/1.1 200 OK",
            r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[{"speaker":"Tim Cook","title":"CEO","content":"Hello","sentiment":0.5}]}"#,
            Arc::clone(&calls),
            1,
        );

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            base_url,
            Some(cache),
        );

        let result = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("api fallback");

        assert!(matches!(result, TranscriptFetch::Found(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(client.cache_failure_count() > 0);
    }

    #[tokio::test]
    async fn fetch_transcript_not_published_result_is_not_cached() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        let calls = Arc::new(AtomicUsize::new(0));

        let base_url = spawn_transcript_server(
            "HTTP/1.1 200 OK",
            r#"{"symbol":"AAPL","quarter":"2025Q1","transcript":[]}"#,
            Arc::clone(&calls),
            2,
        );

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            base_url,
            Some(cache),
        );

        let first = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("first call");
        let second = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("second call");

        assert_eq!(first, TranscriptFetch::NotPublished);
        assert_eq!(second, TranscriptFetch::NotPublished);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fetch_transcript_throttled_result_is_not_cached() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        let calls = Arc::new(AtomicUsize::new(0));

        let base_url =
            spawn_transcript_server("HTTP/1.1 429 Too Many Requests", "", Arc::clone(&calls), 2);

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            base_url,
            Some(cache),
        );

        let first = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("first call");
        let second = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("second call");

        assert_eq!(first, TranscriptFetch::Throttled);
        assert_eq!(second, TranscriptFetch::Throttled);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fetch_transcript_unavailable_result_is_not_cached() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript-cache.db");
        let cache = TranscriptCacheStore::new(Some(&path)).await.expect("cache");
        let calls = Arc::new(AtomicUsize::new(0));

        let base_url = spawn_transcript_server(
            "HTTP/1.1 503 Service Unavailable",
            "",
            Arc::clone(&calls),
            2,
        );

        let client = AlphaVantageClient::new_with_base_url(
            SecretString::from("test-dummy-key"),
            SharedRateLimiter::disabled("test"),
            base_url,
            Some(cache),
        );

        let first = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("first call");
        let second = client
            .fetch_transcript("AAPL", "2025Q1")
            .await
            .expect("second call");

        assert_eq!(first, TranscriptFetch::Unavailable);
        assert_eq!(second, TranscriptFetch::Unavailable);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
