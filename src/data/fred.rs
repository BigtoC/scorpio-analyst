//! FRED (Federal Reserve Economic Data) API client.
//!
//! Provides typed async methods for fetching macro-economic time-series
//! observations from the FRED v2 API.  Uses the `fred/v2/release/observations`
//! endpoint with `Authorization: Bearer` header authentication and cursor-based
//! pagination.  Replaces the paid Finnhub `economic().data()` endpoint for
//! interest-rate and inflation indicators.

use std::time::Duration;

use reqwest::Client;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    config::ApiConfig,
    error::TradingError,
    rate_limit::SharedRateLimiter,
    state::{ImpactDirection, MacroEvent},
};

use super::finnhub::EmptyObjectArgs;

/// Base URL for the FRED v2 `release/observations` endpoint.
///
/// The v2 API groups observations by series within a release and uses
/// cursor-based pagination (`has_more` / `next_cursor`).
const FRED_V2_RELEASE_URL: &str = "https://api.stlouisfed.org/fred/v2/release/observations";

/// FRED release ID for "H.15 Selected Interest Rates" (contains `FEDFUNDS`).
const RELEASE_INTEREST_RATES: u32 = 18;
/// FRED release ID for "Main Economic Indicators" (contains `CPALTT01USM657N`).
const RELEASE_MEI: u32 = 205;

/// FRED series ID for the Federal Funds Effective Rate.
const SERIES_FEDFUNDS: &str = "FEDFUNDS";
/// FRED series ID for the CPI: Total All Items for the US.
const SERIES_CPALTT01: &str = "CPALTT01USM657N";

/// Async client for the FRED (Federal Reserve Economic Data) v2 API.
///
/// Fetches time-series observations via the `fred/v2/release/observations`
/// endpoint.  All outbound requests are gated behind a
/// [`SharedRateLimiter`] and all errors are mapped to [`TradingError`].
#[derive(Clone)]
pub struct FredClient {
    http: Client,
    api_key: SecretString,
    limiter: SharedRateLimiter,
}

impl std::fmt::Debug for FredClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FredClient")
            .field("limiter", &self.limiter.label())
            .finish()
    }
}

impl FredClient {
    /// Create a new client from configuration and a shared rate limiter.
    ///
    /// Returns `Err` when `api.fred_api_key` is not set.
    ///
    /// The HTTP client is configured with `http1_only()` because the FRED CDN
    /// (Akamai) sends HTTP/2 `RST_STREAM INTERNAL_ERROR` to the hyper/h2
    /// client.  Forcing HTTP/1.1 avoids this.
    pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let key = api.fred_api_key.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("SCORPIO_FRED_API_KEY is not set"))
        })?;
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .http1_only()
                .build()
                .map_err(|e| TradingError::Config(anyhow::anyhow!("reqwest client build: {e}")))?,
            api_key: key.clone(),
            limiter,
        })
    }

    /// Construct a non-functional client for use in tests only.
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self {
            http: Client::new(),
            api_key: SecretString::from("test-dummy-key"),
            limiter: SharedRateLimiter::new("test-fred", 30),
        }
    }

    /// Maximum number of attempts for transient network errors.
    const MAX_ATTEMPTS: u32 = 3;
    /// Base delay between retries (multiplied by attempt number).
    const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

    /// Fetch the most recent observation for a given FRED series from a release.
    ///
    /// Returns `Ok(None)` when the series exists but the latest value is
    /// missing (reported as `"."`), or the target series was not found within
    /// the paginated release response.  Returns `Err` on network/parse
    /// failures.
    ///
    /// Retries up to [`Self::MAX_ATTEMPTS`] times on transient connection or
    /// timeout errors with linear backoff.
    pub async fn get_release_series_latest(
        &self,
        release_id: u32,
        target_series: &str,
    ) -> Result<Option<f64>, TradingError> {
        self.limiter.acquire().await;

        let mut last_err = None;
        for attempt in 0..Self::MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Self::RETRY_BASE_DELAY * attempt).await;
                tracing::debug!(attempt, release_id, target_series, "retrying FRED request");
            }
            match self.send_release_request(release_id, target_series).await {
                Ok(val) => return Ok(val),
                Err(e) if is_retryable_fred_err(&e) => {
                    tracing::warn!(
                        attempt,
                        release_id,
                        target_series,
                        error = %e,
                        "transient FRED error, will retry"
                    );
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.expect("loop executed at least once"))
    }

    /// Inner HTTP send + paginate logic for the v2 `release/observations`
    /// endpoint.
    ///
    /// Pages through the release response using `next_cursor` until the target
    /// series is found.  Returns the latest observation value for that series,
    /// or `None` if the series was not found or its latest value is missing.
    async fn send_release_request(
        &self,
        release_id: u32,
        target_series: &str,
    ) -> Result<Option<f64>, TradingError> {
        let release_id_str = release_id.to_string();
        let mut cursor: Option<String> = None;

        loop {
            let mut request = self
                .http
                .get(FRED_V2_RELEASE_URL)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.api_key.expose_secret()),
                )
                .query(&[("release_id", release_id_str.as_str()), ("format", "json")]);

            if let Some(ref c) = cursor {
                request = request.query(&[("next_cursor", c.as_str())]);
            }

            let resp = request
                .send()
                .await
                .map_err(map_fred_err)?
                .error_for_status()
                .map_err(map_fred_err)?
                .json::<FredV2ReleaseResponse>()
                .await
                .map_err(map_fred_err)?;

            // Search for our target series in this page.
            if let Some(entry) = resp.series.iter().find(|s| s.series_id == target_series) {
                // Take the last observation (most recent, since observations
                // are ordered chronologically within a series).
                return Ok(entry.observations.last().and_then(|o| o.parse_value()));
            }

            // If the cursor has already passed our target alphabetically and
            // there are more pages, continue; otherwise give up.
            if resp.has_more {
                cursor = resp.next_cursor;
            } else {
                tracing::warn!(
                    release_id,
                    target_series,
                    "target series not found in release"
                );
                return Ok(None);
            }
        }
    }

    /// Fetch a small, fixed macro-economic snapshot from FRED.
    ///
    /// Replaces the former Finnhub `economic().data()` calls.  Fetches the
    /// Federal Funds Rate (`FEDFUNDS` from release 18) and the CPI Total All
    /// Items for the US (`CPALTT01USM657N` from release 205) concurrently,
    /// then classifies each into a [`MacroEvent`] with an impact direction and
    /// confidence score.
    pub async fn get_economic_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
        let interest_fut = self.get_release_series_latest(RELEASE_INTEREST_RATES, SERIES_FEDFUNDS);
        let inflation_fut = self.get_release_series_latest(RELEASE_MEI, SERIES_CPALTT01);

        let (interest_result, inflation_result) = tokio::join!(interest_fut, inflation_fut);
        let interest_value = interest_result?;
        let inflation_value = inflation_result?;

        let mut events = Vec::new();

        if let Some(direction) = classify_interest_rate(interest_value) {
            events.push(MacroEvent {
                event: "Interest-rate policy shift".to_owned(),
                impact_direction: direction,
                confidence: 0.7,
            });
        }

        if let Some(direction) = classify_inflation(inflation_value) {
            events.push(MacroEvent {
                event: "Inflation signal".to_owned(),
                impact_direction: direction,
                confidence: 0.7,
            });
        }

        Ok(events)
    }
}

// ─── rig::tool::Tool wrapper ─────────────────────────────────────────────────

/// `rig` tool: fetch a fixed macro-economic indicator snapshot from FRED.
///
/// Replaces the former Finnhub-backed `GetEconomicIndicators` tool. Uses
/// the free FRED v2 API to fetch the Federal Funds Rate and CPI data.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetEconomicIndicators {
    #[serde(skip)]
    pub(crate) client: Option<FredClient>,
}

impl GetEconomicIndicators {
    #[must_use]
    pub fn new(client: FredClient) -> Self {
        Self {
            client: Some(client),
        }
    }
}

impl Tool for GetEconomicIndicators {
    const NAME: &'static str = "get_economic_indicators";

    type Error = TradingError;
    type Args = EmptyObjectArgs;
    type Output = Vec<MacroEvent>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_owned(),
            description: "Fetch macro-economic indicators (Federal Funds Rate and CPI) from FRED and summarize them as macro events.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = self.client.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "FredClient not set on GetEconomicIndicators tool"
            ))
        })?;
        client.get_economic_indicators().await
    }
}

// ─── Response types ──────────────────────────────────────────────────────────

/// Raw JSON response from the FRED v2 `release/observations` endpoint.
///
/// The top-level object contains release metadata, a `series` array of
/// per-series observation groups, and cursor-based pagination fields.
#[derive(Debug, Deserialize)]
pub(crate) struct FredV2ReleaseResponse {
    /// Per-series observation groups returned in this page.
    pub series: Vec<FredV2SeriesEntry>,
    /// Whether more pages of data exist beyond this response.
    #[serde(default)]
    pub has_more: bool,
    /// Cursor for the next page (format: `"SERIES_ID,DATE"`).
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A single series within a v2 `release/observations` response.
#[derive(Debug, Deserialize)]
pub(crate) struct FredV2SeriesEntry {
    pub series_id: String,
    #[allow(dead_code)]
    pub title: Option<String>,
    pub observations: Vec<FredObservation>,
}

/// A single FRED observation (date + string-encoded value).
#[derive(Debug, Deserialize)]
pub(crate) struct FredObservation {
    #[allow(dead_code)]
    pub date: String,
    /// Value as a string — FRED uses `"."` for missing observations.
    pub value: String,
}

impl FredObservation {
    /// Parse the string value to `f64`, returning `None` for missing (`"."`)
    /// or unparseable values.
    pub fn parse_value(&self) -> Option<f64> {
        if self.value == "." {
            return None;
        }
        self.value.parse::<f64>().ok()
    }
}

fn map_fred_err(err: reqwest::Error) -> TradingError {
    if err.is_timeout() {
        return TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "FRED request timed out".to_owned(),
        };
    }
    if err.is_status()
        && err
            .status()
            .is_some_and(|s| s == reqwest::StatusCode::TOO_MANY_REQUESTS)
    {
        return TradingError::RateLimitExceeded {
            provider: "fred".to_owned(),
        };
    }
    TradingError::AnalystError {
        agent: "fred".to_owned(),
        message: format!("FRED request failed: {err}"),
    }
}

/// Returns `true` for transient errors worth retrying (connection failures,
/// timeouts).  HTTP 4xx/5xx status errors are *not* retried.
fn is_retryable_fred_err(err: &TradingError) -> bool {
    matches!(
        err,
        TradingError::NetworkTimeout { .. }
            | TradingError::AnalystError {
                agent: _,
                message: _,
            }
    ) && !matches!(err, TradingError::RateLimitExceeded { .. })
}

/// Classify the latest interest rate value into an impact direction.
fn classify_interest_rate(value: Option<f64>) -> Option<ImpactDirection> {
    value.map(|v| {
        if v > 3.0 {
            ImpactDirection::Negative
        } else {
            ImpactDirection::Positive
        }
    })
}

/// Classify the latest inflation (CPI) value into an impact direction.
fn classify_inflation(value: Option<f64>) -> Option<ImpactDirection> {
    value.map(|v| {
        if v > 3.0 {
            ImpactDirection::Negative
        } else {
            ImpactDirection::Positive
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interest_rate_above_3_is_negative() {
        let direction = classify_interest_rate(Some(4.5));
        assert_eq!(direction, Some(ImpactDirection::Negative));
    }

    #[test]
    fn interest_rate_at_or_below_3_is_positive() {
        let direction = classify_interest_rate(Some(2.5));
        assert_eq!(direction, Some(ImpactDirection::Positive));
    }

    #[test]
    fn interest_rate_none_returns_none() {
        let direction = classify_interest_rate(None);
        assert!(direction.is_none());
    }

    #[test]
    fn inflation_above_3_is_negative() {
        let direction = classify_inflation(Some(5.0));
        assert_eq!(direction, Some(ImpactDirection::Negative));
    }

    #[test]
    fn inflation_at_or_below_3_is_positive() {
        let direction = classify_inflation(Some(2.0));
        assert_eq!(direction, Some(ImpactDirection::Positive));
    }

    #[test]
    fn fred_client_new_without_key_returns_config_error() {
        let api = ApiConfig::default();
        let limiter = SharedRateLimiter::new("test-fred", 10);
        let result = FredClient::new(&api, limiter);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    #[test]
    fn deserialize_v2_release_response() {
        let json = r#"{
            "has_more": false,
            "next_cursor": null,
            "release": {
                "release_id": 18,
                "name": "H.15 Selected Interest Rates",
                "url": "http://www.federalreserve.gov/releases/h15/",
                "sources": [{"name": "Board of Governors", "url": "https://www.federalreserve.gov/"}]
            },
            "series": [
                {
                    "series_id": "FEDFUNDS",
                    "title": "Federal Funds Effective Rate",
                    "frequency": "Monthly",
                    "units": "Percent",
                    "seasonal_adjustment": "Not Seasonally Adjusted",
                    "last_updated": "2026-04-01T10:00:00Z",
                    "observations": [
                        {"date": "2026-03-01", "value": "3.64"}
                    ]
                }
            ]
        }"#;
        let resp: FredV2ReleaseResponse = serde_json::from_str(json).expect("should parse");
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
        assert_eq!(resp.series.len(), 1);
        assert_eq!(resp.series[0].series_id, "FEDFUNDS");
        assert_eq!(resp.series[0].observations.len(), 1);
        assert_eq!(resp.series[0].observations[0].date, "2026-03-01");
        assert_eq!(resp.series[0].observations[0].value, "3.64");
    }

    #[test]
    fn deserialize_v2_release_response_with_pagination() {
        let json = r#"{
            "has_more": true,
            "next_cursor": "FEDFUNDS,2026-02-01",
            "series": [
                {
                    "series_id": "CD1M",
                    "observations": [
                        {"date": "2026-01-01", "value": "4.50"}
                    ]
                }
            ]
        }"#;
        let resp: FredV2ReleaseResponse = serde_json::from_str(json).expect("should parse");
        assert!(resp.has_more);
        assert_eq!(resp.next_cursor.as_deref(), Some("FEDFUNDS,2026-02-01"));
        assert_eq!(resp.series[0].series_id, "CD1M");
    }

    #[test]
    fn deserialize_v2_release_response_minimal() {
        let json = r#"{
            "series": [
                {
                    "series_id": "CPALTT01USM657N",
                    "observations": [
                        {"date": "2026-02-01", "value": "3.12"}
                    ]
                }
            ]
        }"#;
        let resp: FredV2ReleaseResponse = serde_json::from_str(json).expect("should parse");
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
        assert_eq!(resp.series[0].series_id, "CPALTT01USM657N");
    }

    #[test]
    fn find_target_series_in_multi_series_response() {
        let json = r#"{
            "has_more": false,
            "series": [
                {
                    "series_id": "CD1M",
                    "observations": [{"date": "2026-03-01", "value": "4.50"}]
                },
                {
                    "series_id": "FEDFUNDS",
                    "observations": [
                        {"date": "2026-02-01", "value": "3.50"},
                        {"date": "2026-03-01", "value": "3.64"}
                    ]
                },
                {
                    "series_id": "TB3MS",
                    "observations": [{"date": "2026-03-01", "value": "4.20"}]
                }
            ]
        }"#;
        let resp: FredV2ReleaseResponse = serde_json::from_str(json).expect("should parse");
        let entry = resp
            .series
            .iter()
            .find(|s| s.series_id == "FEDFUNDS")
            .expect("FEDFUNDS must be present");
        // Latest observation is the last one (chronological order).
        let latest = entry.observations.last().and_then(|o| o.parse_value());
        assert_eq!(latest, Some(3.64));
    }

    #[test]
    fn parse_observation_value_normal() {
        let obs = FredObservation {
            date: "2026-03-01".to_owned(),
            value: "4.33".to_owned(),
        };
        assert_eq!(obs.parse_value(), Some(4.33));
    }

    #[test]
    fn parse_observation_value_missing_dot() {
        let obs = FredObservation {
            date: "2026-03-01".to_owned(),
            value: ".".to_owned(),
        };
        assert_eq!(obs.parse_value(), None);
    }

    #[test]
    fn fred_client_debug_impl() {
        let client = FredClient::for_test();
        let debug = format!("{:?}", client);
        assert!(debug.contains("FredClient"));
        assert!(debug.contains("test-fred"));
    }

    #[tokio::test]
    async fn get_economic_indicators_tool_name() {
        use rig::tool::Tool;
        let tool = GetEconomicIndicators { client: None };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_economic_indicators");
    }

    #[tokio::test]
    async fn get_economic_indicators_accepts_empty_object_args_at_tool_boundary() {
        use rig::tool::Tool;
        let tool = GetEconomicIndicators { client: None };
        let result = tool.call(EmptyObjectArgs {}).await;
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    #[tokio::test]
    async fn get_economic_indicators_definition_advertises_empty_object_schema() {
        use rig::tool::Tool;
        let tool = GetEconomicIndicators { client: None };
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "get_economic_indicators");
        assert_eq!(def.parameters["type"], "object");
        let props = &def.parameters["properties"];
        assert!(
            props.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "properties must be an empty object, got: {props}"
        );
        assert_eq!(
            def.parameters["additionalProperties"], false,
            "additionalProperties must be false"
        );
    }
}
