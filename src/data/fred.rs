//! FRED (Federal Reserve Economic Data) API client.
//!
//! Provides typed async methods for fetching macro-economic time-series
//! observations from the FRED v2 API.  Uses the `fred/v2/series/observations`
//! endpoint with header-based authentication.  Replaces the paid Finnhub
//! `economic().data()` endpoint for interest-rate and inflation indicators.

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

/// Base URL for the FRED v2 API series observations endpoint.
const FRED_BASE_URL: &str = "https://api.stlouisfed.org/fred/v2/series/observations";

/// FRED series ID for the Federal Funds Effective Rate.
const SERIES_FEDFUNDS: &str = "FEDFUNDS";
/// FRED series ID for the CPI: Total All Items for the US.
const SERIES_CPALTT01: &str = "CPALTT01USM657N";

/// Async client for the FRED (Federal Reserve Economic Data) API.
///
/// Fetches time-series observations via the `fred/series/observations`
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
    pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let key = api.fred_api_key.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("SCORPIO_FRED_API_KEY is not set"))
        })?;
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
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

    /// Fetch the most recent observation for a given FRED series.
    ///
    /// Returns `Ok(None)` when the series exists but the latest value is
    /// missing (reported as `"."`).  Returns `Err` on network/parse failures.
    ///
    /// Retries up to [`Self::MAX_ATTEMPTS`] times on transient connection or
    /// timeout errors with linear backoff.
    pub async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
        self.limiter.acquire().await;

        let mut last_err = None;
        for attempt in 0..Self::MAX_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Self::RETRY_BASE_DELAY * attempt).await;
                tracing::debug!(attempt, series_id, "retrying FRED request");
            }
            match self.send_series_request(series_id).await {
                Ok(val) => return Ok(val),
                Err(e) if is_retryable_fred_err(&e) => {
                    tracing::warn!(attempt, series_id, error = %e, "transient FRED error, will retry");
                    last_err = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.expect("loop executed at least once"))
    }

    /// Inner HTTP send + parse logic, extracted for retry.
    ///
    /// Uses the FRED v2 API with header-based authentication.
    async fn send_series_request(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
        let resp = self
            .http
            .get(FRED_BASE_URL)
            .header("X-Api-Key", self.api_key.expose_secret())
            .query(&[
                ("series_id", series_id),
                ("format", "json"),
                ("sort_order", "desc"),
                ("limit", "1"),
            ])
            .send()
            .await
            .map_err(map_fred_err)?
            .error_for_status()
            .map_err(map_fred_err)?
            .json::<FredV2SeriesResponse>()
            .await
            .map_err(map_fred_err)?;

        Ok(resp.observations.first().and_then(|o| o.parse_value()))
    }

    /// Fetch a small, fixed macro-economic snapshot from FRED.
    ///
    /// Replaces the former Finnhub `economic().data()` calls.  Fetches the
    /// Federal Funds Rate (`FEDFUNDS`) and the CPI Total All Items for the US
    /// (`CPALTT01USM657N`) concurrently, then classifies each into a
    /// [`MacroEvent`] with an impact direction and confidence score.
    pub async fn get_economic_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
        let interest_fut = self.get_series_latest(SERIES_FEDFUNDS);
        let inflation_fut = self.get_series_latest(SERIES_CPALTT01);

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
/// the free FRED API to fetch the Federal Funds Rate and CPI data.
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

/// Raw JSON response from the FRED v2 `series/observations` endpoint.
///
/// The v2 response wraps observations with series-level metadata and
/// cursor-based pagination fields (`has_more`, `next_cursor`).
#[derive(Debug, Deserialize)]
pub(crate) struct FredV2SeriesResponse {
    #[allow(dead_code)]
    pub series_id: String,
    pub observations: Vec<FredObservation>,
    /// Whether more pages of data exist beyond this response.
    #[serde(default)]
    #[allow(dead_code)]
    pub has_more: bool,
    /// Cursor token for fetching the next page (format: `"SERIES_ID,DATE"`).
    #[serde(default)]
    #[allow(dead_code)]
    pub next_cursor: Option<String>,
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
    fn deserialize_fred_v2_series_response() {
        let json = r#"{
            "series_id": "FEDFUNDS",
            "observations": [
                {
                    "date": "2026-03-01",
                    "value": "4.33"
                }
            ],
            "has_more": false,
            "next_cursor": null
        }"#;
        let resp: FredV2SeriesResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.series_id, "FEDFUNDS");
        assert_eq!(resp.observations.len(), 1);
        assert_eq!(resp.observations[0].date, "2026-03-01");
        assert_eq!(resp.observations[0].value, "4.33");
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
    }

    #[test]
    fn deserialize_fred_v2_response_with_pagination() {
        let json = r#"{
            "series_id": "FEDFUNDS",
            "observations": [
                { "date": "2026-03-01", "value": "4.33" }
            ],
            "has_more": true,
            "next_cursor": "FEDFUNDS,2026-02-01"
        }"#;
        let resp: FredV2SeriesResponse = serde_json::from_str(json).expect("should parse");
        assert!(resp.has_more);
        assert_eq!(resp.next_cursor.as_deref(), Some("FEDFUNDS,2026-02-01"));
    }

    #[test]
    fn deserialize_fred_v2_response_minimal() {
        let json = r#"{
            "series_id": "CPALTT01USM657N",
            "observations": [
                { "date": "2026-02-01", "value": "3.12" }
            ]
        }"#;
        let resp: FredV2SeriesResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.series_id, "CPALTT01USM657N");
        assert!(!resp.has_more);
        assert!(resp.next_cursor.is_none());
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
