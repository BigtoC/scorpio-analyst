//! FRED (Federal Reserve Economic Data) API client.
//!
//! Provides typed async methods for fetching macroeconomic time-series
//! observations from the FRED API. Uses the `fred/series/observations`
//! endpoint with query-parameter authentication and latest-only lookups.
//! Replaces the paid Finnhub `economic().data()` endpoint for interest-rate
//! and inflation indicators.

use std::time::Duration;

use chrono::{DateTime, Utc};
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

const FRED_BASE_URL: &str = "https://api.stlouisfed.org";
const FRED_V1_SERIES_OBSERVATIONS_PATH: &str = "/fred/series/observations";
const FRED_USER_AGENT: &str = "curl/8.7.1";

/// FRED series ID for the Federal Funds Effective Rate.
const SERIES_FEDFUNDS: &str = "FEDFUNDS";
/// FRED series ID for the CPI: Total All Items for the US.
const SERIES_CPALTT01: &str = "CPALTT01USM657N";

#[derive(Debug, PartialEq, Eq)]
struct FredSeriesQuery<'a> {
    path: &'static str,
    params: [(&'static str, &'a str); 4],
}

fn series_observations_query(series_id: &str) -> FredSeriesQuery<'_> {
    FredSeriesQuery {
        path: FRED_V1_SERIES_OBSERVATIONS_PATH,
        params: [
            ("series_id", series_id),
            ("file_type", "json"),
            ("sort_order", "desc"),
            ("limit", "1"),
        ],
    }
}

/// Async client for the FRED (Federal Reserve Economic Data) API.
///
/// Fetches time-series observations via the `fred/series/observations`
/// endpoint. All outbound requests are gated behind a [`SharedRateLimiter`]
/// and all errors are mapped to [`TradingError`].
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
    /// client. Forcing HTTP/1.1 avoids this. We also send a curl-like user
    /// agent because the same Akamai edge has been observed to stall reqwest's
    /// default user agent from this environment while responding normally to
    /// curl.
    pub fn new(api: &ApiConfig, limiter: SharedRateLimiter) -> Result<Self, TradingError> {
        let key = api.fred_api_key.as_ref().ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!("SCORPIO_FRED_API_KEY is not set"))
        })?;
        Ok(Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .http1_only()
                .user_agent(FRED_USER_AGENT)
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
    /// Upper bound for the full retry loop, including limiter waits and backoff.
    const TOTAL_RETRY_BUDGET: Duration = Duration::from_secs(45);

    /// Fetch the latest observation for a FRED series.
    ///
    /// Returns `Ok(None)` when the latest value is missing (`"."`) or no
    /// observation is present. Retries transient failures with linear backoff.
    pub async fn get_series_latest(&self, series_id: &str) -> Result<Option<f64>, TradingError> {
        self.get_series_latest_classified(series_id)
            .await
            .map_err(Into::into)
    }

    async fn get_series_latest_classified(
        &self,
        series_id: &str,
    ) -> Result<Option<f64>, FredRequestError> {
        let started_at = tokio::time::Instant::now();
        let deadline = started_at + Self::TOTAL_RETRY_BUDGET;
        let mut last_err = None;

        for attempt in 0..Self::MAX_ATTEMPTS {
            if !within_retry_budget(tokio::time::Instant::now(), deadline, None) {
                last_err = Some(budget_exceeded_terminal_error(last_err.as_ref()));
                break;
            }

            if tokio::time::timeout_at(deadline, self.limiter.acquire())
                .await
                .is_err()
            {
                last_err = Some(budget_exceeded_terminal_error(last_err.as_ref()));
                break;
            }

            if attempt > 0 {
                tracing::debug!(attempt, series_id, "retrying FRED request");
            }

            match tokio::time::timeout_at(deadline, self.send_series_request(series_id)).await {
                Ok(Ok(val)) => return Ok(val),
                Ok(Err(error)) => {
                    let decision = classify_retry_decision(&error);

                    if !decision.retryable {
                        return Err(error);
                    }

                    last_err = Some(error);

                    if attempt + 1 == Self::MAX_ATTEMPTS {
                        break;
                    }

                    let retry_delay = compute_retry_delay(attempt + 1, decision.delay_override);
                    if !within_retry_budget(
                        tokio::time::Instant::now(),
                        deadline,
                        Some(retry_delay),
                    ) {
                        last_err = Some(budget_exceeded_terminal_error(last_err.as_ref()));
                        break;
                    }

                    tracing::warn!(
                        attempt,
                        series_id,
                        error = %TradingError::from(last_err.as_ref().expect("retry error stored").clone()),
                        retry_delay_ms = retry_delay.as_millis(),
                        "transient FRED error, will retry"
                    );
                    tokio::time::sleep(retry_delay).await;
                }
                Err(_) => {
                    last_err = Some(budget_exceeded_terminal_error(last_err.as_ref()));
                    break;
                }
            }
        }

        Err(last_err.unwrap_or(FredRequestError::RetryBudgetExhausted))
    }

    async fn send_series_request(&self, series_id: &str) -> Result<Option<f64>, FredRequestError> {
        let query = series_observations_query(series_id);
        let resp = self
            .http
            .get(format!("{FRED_BASE_URL}{}", query.path))
            .query(&query.params)
            .query(&[("api_key", self.api_key.expose_secret())])
            .send()
            .await
            .map_err(map_fred_err)?;

        let resp = map_fred_response_status(resp)?;
        let body = resp
            .text()
            .await
            .map_err(map_fred_err)
            .and_then(|body| decode_series_observations_response(&body))?;

        body.observations
            .first()
            .map(FredObservation::parse_value)
            .transpose()
            .map(Option::flatten)
    }

    /// Fetch a small, fixed macro-economic snapshot from FRED.
    ///
    /// Replaces the former Finnhub `economic().data()` calls. Fetches the
    /// Federal Funds Rate (`FEDFUNDS`) and CPI Total All Items for the US
    /// (`CPALTT01USM657N`) concurrently, then classifies each into a
    /// [`MacroEvent`] with an impact direction and confidence score.
    pub async fn get_economic_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
        let interest_fut = self.get_series_latest_classified(SERIES_FEDFUNDS);
        let inflation_fut = self.get_series_latest_classified(SERIES_CPALTT01);

        let (interest_result, inflation_result) = tokio::join!(interest_fut, inflation_fut);

        collect_macro_events_from_series_results(interest_result, inflation_result)
    }
}

fn collect_macro_events_from_series_results(
    interest_result: Result<Option<f64>, FredRequestError>,
    inflation_result: Result<Option<f64>, FredRequestError>,
) -> Result<Vec<MacroEvent>, TradingError> {
    let both_degraded_transiently = matches!(
        (&interest_result, &inflation_result),
        (Err(interest_error), Err(inflation_error))
            if classify_retry_decision(interest_error).degradable
                && classify_retry_decision(inflation_error).degradable
    );

    let interest_value = degrade_transient_series_error(interest_result, "interest")?;
    let inflation_value = degrade_transient_series_error(inflation_result, "inflation")?;

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

    if events.is_empty() && both_degraded_transiently {
        return Err(TradingError::AnalystError {
            agent: "fred".to_owned(),
            message: "FRED macro collection failed: all series degraded transiently".to_owned(),
        });
    }

    Ok(events)
}

fn degrade_transient_series_error(
    result: Result<Option<f64>, FredRequestError>,
    series_name: &str,
) -> Result<Option<f64>, TradingError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) if classify_retry_decision(&error).degradable => {
            tracing::warn!(series = series_name, error = %TradingError::from(error.clone()), "FRED series fetch failed; degrading to partial macro snapshot");
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

// ─── rig::tool::Tool wrapper ─────────────────────────────────────────────────

/// `rig` tool: fetch a fixed macro-economic indicator snapshot from FRED.
///
/// Replaces the former Finnhub-backed `GetEconomicIndicators` tool. Uses
/// the free FRED series-observations API to fetch the Federal Funds Rate and
/// CPI data.
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

#[derive(Debug, Deserialize)]
pub(crate) struct FredSeriesObservationsResponse {
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
    /// and an error for malformed numeric values.
    fn parse_value(&self) -> Result<Option<f64>, FredRequestError> {
        if self.value == "." {
            return Ok(None);
        }

        self.value
            .parse::<f64>()
            .map(Some)
            .map_err(|err| FredRequestError::ObservationValueParse {
                message: err.to_string(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FredRequestError {
    Timeout,
    RetryBudgetExhausted,
    Transport { message: String },
    RateLimited { retry_after: Option<Duration> },
    TransientServer { status: reqwest::StatusCode },
    PermanentClient { status: reqwest::StatusCode },
    Decode { message: String },
    ObservationValueParse { message: String },
}

impl From<FredRequestError> for TradingError {
    fn from(value: FredRequestError) -> Self {
        match value {
            FredRequestError::Timeout => TradingError::NetworkTimeout {
                elapsed: Duration::from_secs(30),
                message: "FRED request timed out".to_owned(),
            },
            FredRequestError::RetryBudgetExhausted => TradingError::NetworkTimeout {
                elapsed: FredClient::TOTAL_RETRY_BUDGET,
                message: "FRED retry budget exhausted".to_owned(),
            },
            FredRequestError::Transport { message } => TradingError::AnalystError {
                agent: "fred".to_owned(),
                message: format!("FRED transport failed: {message}"),
            },
            FredRequestError::RateLimited { .. } => TradingError::RateLimitExceeded {
                provider: "fred".to_owned(),
            },
            FredRequestError::TransientServer { status } => TradingError::AnalystError {
                agent: "fred".to_owned(),
                message: format!("FRED server error: {status}"),
            },
            FredRequestError::PermanentClient { status } => TradingError::AnalystError {
                agent: "fred".to_owned(),
                message: format!("FRED request failed: {status}"),
            },
            FredRequestError::Decode { message } => TradingError::AnalystError {
                agent: "fred".to_owned(),
                message: format!("FRED response decode failed: {message}"),
            },
            FredRequestError::ObservationValueParse { message } => TradingError::AnalystError {
                agent: "fred".to_owned(),
                message: format!("FRED observation value parse failed: {message}"),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FredRetryDecision {
    retryable: bool,
    degradable: bool,
    delay_override: Option<Duration>,
}

fn classify_retry_decision(error: &FredRequestError) -> FredRetryDecision {
    match error {
        FredRequestError::Timeout
        | FredRequestError::Transport { .. }
        | FredRequestError::TransientServer { .. } => FredRetryDecision {
            retryable: true,
            degradable: true,
            delay_override: None,
        },
        FredRequestError::RateLimited { retry_after } => FredRetryDecision {
            retryable: true,
            degradable: true,
            delay_override: *retry_after,
        },
        FredRequestError::RetryBudgetExhausted => FredRetryDecision {
            retryable: false,
            degradable: true,
            delay_override: None,
        },
        FredRequestError::PermanentClient { .. }
        | FredRequestError::Decode { .. }
        | FredRequestError::ObservationValueParse { .. } => FredRetryDecision {
            retryable: false,
            degradable: false,
            delay_override: None,
        },
    }
}

fn compute_retry_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or(FredClient::RETRY_BASE_DELAY * attempt)
}

fn budget_exceeded_terminal_error(last_err: Option<&FredRequestError>) -> FredRequestError {
    last_err
        .filter(|error| classify_retry_decision(error).degradable)
        .cloned()
        .unwrap_or(FredRequestError::RetryBudgetExhausted)
}

fn within_retry_budget(
    now: tokio::time::Instant,
    deadline: tokio::time::Instant,
    upcoming_delay: Option<Duration>,
) -> bool {
    match upcoming_delay {
        Some(delay) => now.checked_add(delay).is_some_and(|wake| wake <= deadline),
        None => now <= deadline,
    }
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    parse_retry_after_at(headers, Utc::now())
}

fn parse_retry_after_at(
    headers: &reqwest::header::HeaderMap,
    now: DateTime<Utc>,
) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let value = value.trim();

            value
                .parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| {
                    DateTime::parse_from_rfc2822(value)
                        .ok()
                        .map(|retry_at| retry_at.with_timezone(&Utc))
                        .and_then(|retry_at| {
                            let delay = retry_at.signed_duration_since(now);
                            if delay <= chrono::Duration::zero() {
                                return Some(Duration::ZERO);
                            }
                            delay.to_std().ok()
                        })
                })
        })
}

fn map_fred_err(err: reqwest::Error) -> FredRequestError {
    if err.is_timeout() {
        return FredRequestError::Timeout;
    }
    FredRequestError::Transport {
        message: err.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FredStatusClass {
    RateLimited,
    TransientServer,
    PermanentClient,
}

fn classify_fred_status(status: reqwest::StatusCode) -> FredStatusClass {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        FredStatusClass::RateLimited
    } else if status.is_server_error() {
        FredStatusClass::TransientServer
    } else {
        FredStatusClass::PermanentClient
    }
}

fn map_fred_response_status(
    resp: reqwest::Response,
) -> Result<reqwest::Response, FredRequestError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }

    Err(match classify_fred_status(status) {
        FredStatusClass::RateLimited => FredRequestError::RateLimited {
            retry_after: parse_retry_after(resp.headers()),
        },
        FredStatusClass::TransientServer => FredRequestError::TransientServer { status },
        FredStatusClass::PermanentClient => FredRequestError::PermanentClient { status },
    })
}

fn map_fred_decode_err(err: serde_json::Error) -> FredRequestError {
    FredRequestError::Decode {
        message: err.to_string(),
    }
}

fn decode_series_observations_response(
    body: &str,
) -> Result<FredSeriesObservationsResponse, FredRequestError> {
    serde_json::from_str(body).map_err(map_fred_decode_err)
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
    fn series_observations_query_targets_v1_endpoint_with_latest_only_params() {
        let query = series_observations_query(SERIES_FEDFUNDS);

        assert_eq!(query.path, "/fred/series/observations");
        assert_eq!(
            query.params,
            [
                ("series_id", "FEDFUNDS"),
                ("file_type", "json"),
                ("sort_order", "desc"),
                ("limit", "1"),
            ]
        );
    }

    #[test]
    fn fred_status_429_maps_to_rate_limit() {
        assert!(matches!(
            classify_fred_status(reqwest::StatusCode::TOO_MANY_REQUESTS),
            FredStatusClass::RateLimited
        ));
    }

    #[test]
    fn fred_status_500_is_retryable_transient() {
        assert!(matches!(
            classify_fred_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            FredStatusClass::TransientServer
        ));
    }

    #[test]
    fn fred_status_401_is_permanent() {
        assert!(matches!(
            classify_fred_status(reqwest::StatusCode::UNAUTHORIZED),
            FredStatusClass::PermanentClient
        ));
    }

    #[test]
    fn fred_status_404_is_permanent() {
        assert!(matches!(
            classify_fred_status(reqwest::StatusCode::NOT_FOUND),
            FredStatusClass::PermanentClient
        ));
    }

    #[test]
    fn retry_decision_marks_rate_limit_as_retryable_and_degradable() {
        let decision = classify_retry_decision(&FredRequestError::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
        });

        assert!(decision.retryable);
        assert!(decision.degradable);
        assert_eq!(decision.delay_override, Some(Duration::from_secs(2)));
    }

    #[test]
    fn retry_decision_marks_decode_failures_as_permanent() {
        let decision = classify_retry_decision(&FredRequestError::Decode {
            message: "bad json".to_owned(),
        });

        assert!(!decision.retryable);
        assert!(!decision.degradable);
        assert_eq!(decision.delay_override, None);
    }

    #[test]
    fn compute_retry_delay_uses_retry_after_header_when_present() {
        let delay = compute_retry_delay(2, Some(Duration::from_secs(7)));

        assert_eq!(delay, Duration::from_secs(7));
    }

    #[test]
    fn compute_retry_delay_uses_linear_backoff_without_retry_after() {
        let delay = compute_retry_delay(2, None);

        assert_eq!(delay, FredClient::RETRY_BASE_DELAY * 2);
    }

    #[test]
    fn within_retry_budget_rejects_sleep_past_deadline() {
        let now = tokio::time::Instant::now();
        let deadline = now + Duration::from_secs(1);

        assert!(!within_retry_budget(
            now,
            deadline,
            Some(Duration::from_secs(2))
        ));
    }

    #[test]
    fn parse_retry_after_seconds_header() {
        let headers = [(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("3"),
        )]
        .into_iter()
        .collect::<reqwest::header::HeaderMap>();

        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(3)));
    }

    #[test]
    fn parse_retry_after_http_date_header() {
        let headers = [(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("Thu, 02 Apr 2026 12:00:03 GMT"),
        )]
        .into_iter()
        .collect::<reqwest::header::HeaderMap>();

        let now = DateTime::parse_from_rfc3339("2026-04-02T12:00:00Z")
            .expect("valid fixed now")
            .with_timezone(&Utc);

        assert_eq!(
            parse_retry_after_at(&headers, now),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn retry_budget_total_is_bounded_above_single_request_timeout() {
        assert!(FredClient::TOTAL_RETRY_BUDGET > Duration::from_secs(30));
        assert!(FredClient::TOTAL_RETRY_BUDGET < Duration::from_secs(90));
    }

    #[test]
    fn retry_budget_terminal_error_preserves_rate_limit_cause_when_next_delay_cannot_fit() {
        let error = FredRequestError::RateLimited {
            retry_after: Some(Duration::from_secs(60)),
        };

        assert_eq!(budget_exceeded_terminal_error(Some(&error)), error);
    }

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
    fn deserialize_v1_series_observations_response() {
        let json = r#"{
            "observations": [
                {"date": "2026-03-01", "value": "4.33"}
            ]
        }"#;
        let resp: FredSeriesObservationsResponse =
            serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.observations.len(), 1);
        assert_eq!(
            resp.observations[0]
                .parse_value()
                .expect("valid numeric value should parse"),
            Some(4.33)
        );
    }

    #[test]
    fn decode_series_observations_response_rejects_malformed_json() {
        let result = decode_series_observations_response("{not-json");

        assert!(matches!(
            result,
            Err(FredRequestError::Decode { ref message }) if !message.is_empty()
        ));
    }

    #[test]
    fn deserialize_v1_series_observations_response_missing_value_maps_to_none() {
        let json = r#"{
            "observations": [
                {"date": "2026-03-01", "value": "."}
            ]
        }"#;
        let resp: FredSeriesObservationsResponse =
            serde_json::from_str(json).expect("should parse");

        assert_eq!(resp.observations.len(), 1);
        assert_eq!(
            resp.observations[0]
                .parse_value()
                .expect("missing marker should parse"),
            None
        );
    }

    #[test]
    fn parse_observation_value_normal() {
        let obs = FredObservation {
            date: "2026-03-01".to_owned(),
            value: "4.33".to_owned(),
        };
        assert_eq!(
            obs.parse_value().expect("valid numeric value should parse"),
            Some(4.33)
        );
    }

    #[test]
    fn parse_observation_value_missing_dot() {
        let obs = FredObservation {
            date: "2026-03-01".to_owned(),
            value: ".".to_owned(),
        };
        assert_eq!(
            obs.parse_value().expect("missing marker should parse"),
            None
        );
    }

    #[test]
    fn parse_observation_value_invalid_number_hard_fails() {
        let obs = FredObservation {
            date: "2026-03-01".to_owned(),
            value: "abc".to_owned(),
        };

        assert!(matches!(
            obs.parse_value(),
            Err(FredRequestError::ObservationValueParse { ref message }) if !message.is_empty()
        ));
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

    #[test]
    fn economic_indicator_collection_degrades_when_series_fetches_fail() {
        let result =
            collect_macro_events_from_series_results(Err(FredRequestError::Timeout), Ok(Some(2.5)))
                .expect("transient FRED failures should degrade, not abort macro collection");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event, "Inflation signal");
        assert_eq!(result[0].impact_direction, ImpactDirection::Positive);
    }

    #[test]
    fn economic_indicator_collection_degrades_transient_server_failures() {
        let result = collect_macro_events_from_series_results(
            Err(FredRequestError::TransientServer {
                status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            }),
            Ok(Some(2.5)),
        )
        .expect("transient server failures should degrade, not abort macro collection");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event, "Inflation signal");
        assert_eq!(result[0].impact_direction, ImpactDirection::Positive);
    }

    #[test]
    fn economic_indicator_collection_errors_when_both_series_degrade_transiently() {
        let result = collect_macro_events_from_series_results(
            Err(FredRequestError::Timeout),
            Err(FredRequestError::TransientServer {
                status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            }),
        );

        assert!(
            result.is_err(),
            "full transient outage must not degrade to Ok([])"
        );
    }

    #[test]
    fn economic_indicator_collection_propagates_non_transient_analyst_failures() {
        let result = collect_macro_events_from_series_results(
            Err(FredRequestError::PermanentClient {
                status: reqwest::StatusCode::UNAUTHORIZED,
            }),
            Ok(Some(2.5)),
        );

        assert!(matches!(result, Err(TradingError::AnalystError { .. })));
    }

    #[test]
    fn economic_indicator_collection_propagates_decode_failures() {
        let result = collect_macro_events_from_series_results(
            Err(FredRequestError::Decode {
                message: "expected value".to_owned(),
            }),
            Ok(Some(2.5)),
        );

        assert!(matches!(result, Err(TradingError::AnalystError { .. })));
    }

    #[test]
    fn economic_indicator_collection_propagates_non_transient_client_failures() {
        let result = collect_macro_events_from_series_results(
            Err(FredRequestError::PermanentClient {
                status: reqwest::StatusCode::BAD_REQUEST,
            }),
            Ok(Some(2.5)),
        );

        assert!(matches!(result, Err(TradingError::AnalystError { .. })));
    }
}
