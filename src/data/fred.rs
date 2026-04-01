//! FRED (Federal Reserve Economic Data) API client.
//!
//! Provides typed async methods for fetching macro-economic time-series
//! observations from the free FRED API.  Replaces the paid Finnhub
//! `economic().data()` endpoint for interest-rate and inflation indicators.

use std::time::Duration;

use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::{
    config::ApiConfig,
    error::TradingError,
    rate_limit::SharedRateLimiter,
};

/// Base URL for the FRED API series observations endpoint.
const FRED_BASE_URL: &str = "https://api.stlouisfed.org/fred/series/observations";

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
            api_key: SecretString::from(key.expose_secret().to_owned()),
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

    /// Fetch the most recent observation for a given FRED series.
    ///
    /// Returns `Ok(None)` when the series exists but the latest value is
    /// missing (reported as `"."`).  Returns `Err` on network/parse failures.
    pub async fn get_series_latest(
        &self,
        series_id: &str,
    ) -> Result<Option<f64>, TradingError> {
        self.limiter.acquire().await;

        let resp = self
            .http
            .get(FRED_BASE_URL)
            .query(&[
                ("series_id", series_id),
                ("api_key", self.api_key.expose_secret()),
                ("file_type", "json"),
                ("sort_order", "desc"),
                ("limit", "1"),
            ])
            .send()
            .await
            .map_err(map_fred_err)?
            .error_for_status()
            .map_err(map_fred_err)?
            .json::<FredObservationsResponse>()
            .await
            .map_err(map_fred_err)?;

        Ok(resp.observations.first().and_then(|o| o.parse_value()))
    }
}

/// Raw JSON response from the FRED `series/observations` endpoint.
///
/// We only deserialize the `observations` array; other metadata fields are
/// ignored via `#[serde(deny_unknown_fields)]` being intentionally absent.
#[derive(Debug, Deserialize)]
pub(crate) struct FredObservationsResponse {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fred_client_new_without_key_returns_config_error() {
        let api = ApiConfig::default();
        let limiter = SharedRateLimiter::new("test-fred", 10);
        let result = FredClient::new(&api, limiter);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Config(_)));
    }

    #[test]
    fn deserialize_fred_observations_response() {
        let json = r#"{
            "realtime_start": "2026-04-01",
            "realtime_end": "2026-04-01",
            "observation_start": "1776-07-04",
            "observation_end": "9999-12-31",
            "units": "lin",
            "output_type": 1,
            "file_type": "json",
            "order_by": "observation_date",
            "sort_order": "desc",
            "count": 1,
            "offset": 0,
            "limit": 1,
            "observations": [
                {
                    "realtime_start": "2026-04-01",
                    "realtime_end": "2026-04-01",
                    "date": "2026-03-01",
                    "value": "4.33"
                }
            ]
        }"#;
        let resp: FredObservationsResponse = serde_json::from_str(json).expect("should parse");
        assert_eq!(resp.observations.len(), 1);
        assert_eq!(resp.observations[0].date, "2026-03-01");
        assert_eq!(resp.observations[0].value, "4.33");
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
}
