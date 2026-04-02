# Replace Finnhub Economic Indicators with FRED API — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the paid Finnhub `economic().data()` calls with the free FRED (Federal Reserve Economic Data) `series/observations` API for fetching macro-economic indicators (interest rate + CPI/inflation).

**Architecture:** Create a standalone `FredClient` in `src/data/fred.rs` that makes HTTP GET requests to `https://api.stlouisfed.org/fred/series/observations`. Move the `GetEconomicIndicators` rig tool from `finnhub.rs` to `fred.rs`, now backed by `FredClient`. Thread `FredClient` through the `NewsAnalyst` → `NewsAnalystTask` → `TradingPipeline` wiring chain.

**Tech Stack:** `reqwest` (HTTP client, with `json` feature), FRED API v1 (`fred/series/observations` endpoint), existing `secrecy`/`serde`/`tokio`/`governor` infrastructure.

---

## Series ID Mapping

| Finnhub Code | Meaning | FRED Series ID |
|---|---|---|
| `MA-USA-656880` | Interest-rate proxy (Federal Funds Rate) | `FEDFUNDS` |
| `MA-USA-CPALTT01-USM657N` | CPI: Total All Items for the US | `CPALTT01USM657N` |

## FRED API Details

- **Endpoint:** `GET https://api.stlouisfed.org/fred/series/observations`
- **Required params:** `api_key` (32-char string), `series_id` (string)
- **Key optional params:** `file_type=json`, `sort_order=desc`, `limit=1` (fetch only latest observation)
- **JSON response shape:**
  ```json
  {
    "observations": [
      { "realtime_start": "...", "realtime_end": "...", "date": "YYYY-MM-DD", "value": "123.45" }
    ]
  }
  ```
- **Note:** `value` is a string (to avoid precision loss). Missing values are reported as `"."`.
- **Free tier limit:** 120 requests per minute.

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| **Create** | `src/data/fred.rs` | `FredClient` struct, `get_series_latest()`, `get_economic_indicators()`, `GetEconomicIndicators` tool |
| **Modify** | `src/data/finnhub.rs:226-296` | Remove `get_economic_indicators()` method |
| **Modify** | `src/data/finnhub.rs:920-963` | Remove `GetEconomicIndicators` struct + `Tool` impl |
| **Modify** | `src/data/finnhub.rs:1325-1370` | Remove `GetEconomicIndicators` tests |
| **Modify** | `src/data/mod.rs` | Add `pub mod fred;`, move `GetEconomicIndicators` re-export from `finnhub` to `fred` |
| **Modify** | `src/config.rs:91-103` | Add `fred_api_key: Option<SecretString>` to `ApiConfig` |
| **Modify** | `src/config.rs:206-221` | Add `fred_api_key` to `Debug` impl |
| **Modify** | `src/config.rs:260-265` | Add `secret_from_env("SCORPIO_FRED_API_KEY")` |
| **Modify** | `src/config.rs:110-130` | Add `fred_rps: u32` to `RateLimitConfig` |
| **Modify** | `src/config.rs:148-158` | Add `fred_rps` to `Default` impl |
| **Modify** | `src/rate_limit.rs:69-77` | Add `fred_from_config()` factory method |
| **Modify** | `src/agents/analyst/news.rs:63-106` | Add `fred: FredClient` field, accept in `new()` |
| **Modify** | `src/agents/analyst/news.rs:121-124` | Use `FredClient`-backed `GetEconomicIndicators` |
| **Modify** | `src/agents/analyst/mod.rs:76-99` | Accept `FredClient` param, pass to `NewsAnalyst` |
| **Modify** | `src/workflow/tasks/analyst.rs:224-242` | Add `fred: FredClient` to `NewsAnalystTask` |
| **Modify** | `src/workflow/pipeline.rs:227-251` | Construct `FredClient`, pass to `NewsAnalystTask` |
| **Modify** | `Cargo.toml:46-50` | Add `reqwest` dependency |
| **Modify** | `.env.example:10-12` | Add `SCORPIO_FRED_API_KEY` |
| **Modify** | `config.toml:26` | Add `fred_rps = 2` |

---

## Chunk 1: Foundation — Config, Dependencies, and FredClient

### Task 1: Add `reqwest` dependency to `Cargo.toml`

**Files:**
- Modify: `Cargo.toml:46-50`

- [ ] **Step 1: Add reqwest with json feature**

  In the `[dependencies]` section under `# Utilities`, add:
  ```toml
  reqwest = { version = "0.12", features = ["json"] }
  ```

- [ ] **Step 2: Verify it compiles**

  Run: `cargo check`
  Expected: compiles with no errors

- [ ] **Step 3: Commit**

  ```bash
  git add Cargo.toml Cargo.lock
  git commit -m "deps: add reqwest for FRED API HTTP client"
  ```

---

### Task 2: Add FRED API key to config layer

**Files:**
- Modify: `src/config.rs:91-103` (ApiConfig)
- Modify: `src/config.rs:110-130` (RateLimitConfig)
- Modify: `src/config.rs:132-146` (default functions)
- Modify: `src/config.rs:148-158` (Default impl)
- Modify: `src/config.rs:206-221` (Debug impl)
- Modify: `src/config.rs:260-265` (secret_from_env)
- Modify: `.env.example:10-12`
- Modify: `config.toml:26`
- Modify: `src/rate_limit.rs:69-77`

- [ ] **Step 1: Add `fred_api_key` to `ApiConfig`**

  In `src/config.rs`, inside `pub struct ApiConfig`, after the `openrouter_api_key` field (line 102), add:
  ```rust
  #[serde(skip)]
  pub fred_api_key: Option<SecretString>,
  ```

- [ ] **Step 2: Add `fred_api_key` to `Debug` impl**

  In the manual `Debug` impl for `ApiConfig` (around line 206), add before `.finish()`:
  ```rust
  .field("fred_api_key", &secret_display(&self.fred_api_key))
  ```

- [ ] **Step 3: Load FRED key from env**

  In `Config::load_from()` (around line 265), add after the `openrouter_api_key` line:
  ```rust
  cfg.api.fred_api_key = secret_from_env("SCORPIO_FRED_API_KEY");
  ```

- [ ] **Step 4: Add `fred_rps` to `RateLimitConfig`**

  In `pub struct RateLimitConfig`, after `finnhub_rps` (line 129), add:
  ```rust
  /// FRED requests per second (0 = disabled; free tier allows ~2 rps).
  #[serde(default = "default_fred_rps")]
  pub fred_rps: u32,
  ```

  Add the default function after `default_finnhub_rps()`:
  ```rust
  fn default_fred_rps() -> u32 {
      2
  }
  ```

  Add to the `Default` impl for `RateLimitConfig`, after `finnhub_rps`:
  ```rust
  fred_rps: default_fred_rps(),
  ```

- [ ] **Step 5: Add `fred_from_config()` to `SharedRateLimiter`**

  In `src/rate_limit.rs`, after `finnhub_from_config()` (line 77), add:
  ```rust
  /// Create a FRED rate limiter from `RateLimitConfig`.
  ///
  /// Returns `None` when `cfg.fred_rps == 0` (disabled).
  pub fn fred_from_config(cfg: &RateLimitConfig) -> Option<Self> {
      if cfg.fred_rps == 0 {
          return None;
      }
      Some(Self::new("fred", cfg.fred_rps))
  }
  ```

- [ ] **Step 6: Update `.env.example`**

  Add after the `SCORPIO_FINNHUB_API_KEY` line:
  ```bash
  SCORPIO_FRED_API_KEY=your-fred-api-key-here
  ```

- [ ] **Step 7: Update `config.toml`**

  In the `[rate_limits]` section, add after `finnhub_rps = 30`:
  ```toml
  fred_rps = 2
  ```

- [ ] **Step 8: Verify it compiles**

  Run: `cargo check`
  Expected: compiles with no errors

- [ ] **Step 9: Commit**

  ```bash
  git add src/config.rs src/rate_limit.rs .env.example config.toml
  git commit -m "config: add FRED API key and rate limit settings"
  ```

---

### Task 3: Create `FredClient` with `get_series_latest()` and unit tests

**Files:**
- Create: `src/data/fred.rs`
- Test: `src/data/fred.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test for `FredClient` construction**

  Create `src/data/fred.rs` with the test module first:
  ```rust
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
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test --lib data::fred::tests::fred_client_new_without_key_returns_config_error`
  Expected: FAIL (struct doesn't exist yet)

- [ ] **Step 3: Implement `FredClient` struct and constructor**

  In `src/data/fred.rs`, add the implementation above the tests:
  ```rust
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
  }
  ```

- [ ] **Step 4: Run test to verify it passes**

  Run: `cargo test --lib data::fred::tests::fred_client_new_without_key_returns_config_error`
  Expected: PASS

- [ ] **Step 5: Write the failing test for response deserialization**

  Add to the test module:
  ```rust
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
  ```

- [ ] **Step 6: Implement FRED response types**

  Add above the test module:
  ```rust
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
  ```

- [ ] **Step 7: Run tests to verify they pass**

  Run: `cargo test --lib data::fred::tests`
  Expected: all 4 tests PASS

- [ ] **Step 8: Write the failing test for `get_series_latest()`**

  Add to the test module:
  ```rust
  #[test]
  fn fred_client_debug_impl() {
      let client = FredClient::for_test();
      let debug = format!("{:?}", client);
      assert!(debug.contains("FredClient"));
      assert!(debug.contains("test-fred"));
  }
  ```

- [ ] **Step 9: Implement `get_series_latest()`**

  Add to `impl FredClient`:
  ```rust
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
  ```

  Add the error mapping function:
  ```rust
  fn map_fred_err(err: reqwest::Error) -> TradingError {
      if err.is_timeout() {
          return TradingError::NetworkTimeout {
              elapsed: Duration::from_secs(30),
              message: "FRED request timed out".to_owned(),
          };
      }
      if err.is_status() {
          if let Some(status) = err.status() {
              if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                  return TradingError::RateLimitExceeded {
                      provider: "fred".to_owned(),
                  };
              }
          }
      }
      TradingError::AnalystError {
          agent: "fred".to_owned(),
          message: format!("FRED request failed: {err}"),
      }
  }
  ```

- [ ] **Step 10: Run all tests**

  Run: `cargo test --lib data::fred::tests`
  Expected: all tests PASS

- [ ] **Step 11: Commit**

  ```bash
  git add src/data/fred.rs
  git commit -m "feat: add FredClient with series observation fetching"
  ```

---

### Task 4: Implement `get_economic_indicators()` on `FredClient`

**Files:**
- Modify: `src/data/fred.rs`

- [ ] **Step 1: Write the failing test for indicator logic**

  Add to the test module in `fred.rs`:
  ```rust
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
  ```

- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test --lib data::fred::tests`
  Expected: FAIL (functions don't exist)

- [ ] **Step 3: Implement classification helpers and `get_economic_indicators()`**

  Add imports at the top of `fred.rs`:
  ```rust
  use crate::state::{ImpactDirection, MacroEvent};
  ```

  Add the classification helpers:
  ```rust
  /// FRED series ID for the Federal Funds Effective Rate.
  const SERIES_FEDFUNDS: &str = "FEDFUNDS";
  /// FRED series ID for the CPI: Total All Items for the US.
  const SERIES_CPALTT01: &str = "CPALTT01USM657N";

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
  ```

  Add the public method to `impl FredClient`:
  ```rust
  /// Fetch a small, fixed macro-economic snapshot from FRED.
  ///
  /// Replaces the former Finnhub `economic().data()` calls.  Fetches the
  /// Federal Funds Rate (`FEDFUNDS`) and the CPI Total All Items for the US
  /// (`CPALTT01USM657N`) concurrently, then classifies each into a
  /// [`MacroEvent`] with an impact direction and confidence score.
  pub async fn get_economic_indicators(&self) -> Result<Vec<MacroEvent>, TradingError> {
      let interest_fut = self.get_series_latest(SERIES_FEDFUNDS);
      let inflation_fut = self.get_series_latest(SERIES_CPALTT01);

      let (interest_result, inflation_result) =
          tokio::join!(interest_fut, inflation_fut);
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
  ```

- [ ] **Step 4: Run tests to verify they pass**

  Run: `cargo test --lib data::fred::tests`
  Expected: all tests PASS

- [ ] **Step 5: Commit**

  ```bash
  git add src/data/fred.rs
  git commit -m "feat: add FRED-backed get_economic_indicators with interest rate and CPI"
  ```

---

### Task 5: Move `GetEconomicIndicators` tool to `fred.rs`

**Files:**
- Modify: `src/data/fred.rs` (add tool)
- Modify: `src/data/finnhub.rs:920-963` (remove tool)
- Modify: `src/data/finnhub.rs:1325-1370` (remove tests)
- Modify: `src/data/mod.rs` (update re-exports)

- [ ] **Step 1: Write the tool tests in `fred.rs`**

  Add to the test module:
  ```rust
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
  ```

- [ ] **Step 2: Implement `GetEconomicIndicators` tool in `fred.rs`**

  Add the necessary imports:
  ```rust
  use rig::completion::ToolDefinition;
  use rig::tool::Tool;
  use schemars::JsonSchema;
  use serde::{Serialize, Deserialize};
  use serde_json::json;
  ```

  Add the `EmptyObjectArgs` re-use (import from finnhub) or define locally. Prefer importing:
  ```rust
  use super::finnhub::EmptyObjectArgs;
  ```

  Add the tool struct:
  ```rust
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
  ```

- [ ] **Step 3: Run the new tests**

  Run: `cargo test --lib data::fred::tests`
  Expected: all tests PASS

- [ ] **Step 4: Remove `GetEconomicIndicators` from `finnhub.rs`**

  Remove from `src/data/finnhub.rs`:
  1. Lines 226-296: the `get_economic_indicators()` method on `FinnhubClient` (including the `INTEREST_RATE_CODE` and `INFLATION_CODE` constants and the `push_macro_event` helper — but **keep** `push_macro_event` if it's used elsewhere in the file; check first — it IS used by `derive_macro_events` at line 558/569/586, so keep it).
  2. Lines 920-963: the `GetEconomicIndicators` struct and `Tool` impl.
  3. Lines 1325-1370: the two `GetEconomicIndicators` tests (`get_economic_indicators_accepts_empty_object_args_at_tool_boundary` and `get_economic_indicators_definition_advertises_empty_object_schema`).
  4. Remove the `ImpactDirection` and `MacroEvent` imports from the `use crate::state::{...}` block **only if** they are no longer used in `finnhub.rs`. (`ImpactDirection` IS still used in `derive_macro_events`; `MacroEvent` IS still used in `push_macro_event` and `derive_macro_events`. So keep both.)

- [ ] **Step 5: Update `src/data/mod.rs` re-exports**

  Change:
  ```rust
  pub mod finnhub;
  mod symbol;
  pub mod yfinance;
  ```
  To:
  ```rust
  pub mod finnhub;
  pub mod fred;
  mod symbol;
  pub mod yfinance;
  ```

  Change the `finnhub` re-export to remove `GetEconomicIndicators`:
  ```rust
  pub use finnhub::{
      FinnhubClient, GetCachedNews, GetEarnings, GetFundamentals,
      GetInsiderTransactions, GetMarketNews, GetNews, SymbolArgs,
  };
  ```

  Add the `fred` re-export:
  ```rust
  pub use fred::{FredClient, GetEconomicIndicators};
  ```

  Update the module doc table to reflect the new home of `GetEconomicIndicators` and add `FredClient`.

- [ ] **Step 6: Verify full build compiles**

  Run: `cargo check`
  Expected: may have compile errors in downstream files (news.rs, tasks, pipeline) — that's expected; they will be fixed in Chunk 2.

- [ ] **Step 7: Commit**

  ```bash
  git add src/data/fred.rs src/data/finnhub.rs src/data/mod.rs
  git commit -m "feat: move GetEconomicIndicators tool from Finnhub to FRED-backed client"
  ```

---

## Chunk 2: Wiring — Thread FredClient through the agent/workflow layers

### Task 6: Update `NewsAnalyst` to accept `FredClient`

**Files:**
- Modify: `src/agents/analyst/news.rs:13-19` (imports)
- Modify: `src/agents/analyst/news.rs:63-106` (struct + constructor)
- Modify: `src/agents/analyst/news.rs:121-124` (tool construction)

- [ ] **Step 1: Update imports in `news.rs`**

  Change the data import from:
  ```rust
  use crate::data::{FinnhubClient, GetCachedNews, GetEconomicIndicators, GetMarketNews, GetNews};
  ```
  To:
  ```rust
  use crate::data::{
      FinnhubClient, FredClient, GetCachedNews, GetEconomicIndicators, GetMarketNews, GetNews,
  };
  ```

- [ ] **Step 2: Add `fred` field to `NewsAnalyst` struct**

  Add after the `finnhub: FinnhubClient` field:
  ```rust
  fred: FredClient,
  ```

- [ ] **Step 3: Update `NewsAnalyst::new()` signature**

  Add `fred: FredClient` parameter after the `finnhub` parameter:
  ```rust
  pub fn new(
      handle: CompletionModelHandle,
      finnhub: FinnhubClient,
      fred: FredClient,
      symbol: impl Into<String>,
      target_date: impl Into<String>,
      llm_config: &LlmConfig,
      cached_news: Option<Arc<NewsData>>,
  ) -> Self {
  ```

  And assign it in the struct literal:
  ```rust
  Self {
      handle,
      finnhub,
      fred,
      symbol: runtime.symbol,
      ...
  }
  ```

- [ ] **Step 4: Use `FredClient` for `GetEconomicIndicators` in `run()`**

  Change line 124 from:
  ```rust
  Box::new(GetEconomicIndicators::new(self.finnhub.clone())),
  ```
  To:
  ```rust
  Box::new(GetEconomicIndicators::new(self.fred.clone())),
  ```

- [ ] **Step 5: Verify module compiles in isolation**

  Run: `cargo check`
  Expected: errors in callers of `NewsAnalyst::new()` (mod.rs and tasks) — expected.

- [ ] **Step 6: Commit**

  ```bash
  git add src/agents/analyst/news.rs
  git commit -m "refactor: NewsAnalyst accepts FredClient for economic indicators"
  ```

---

### Task 7: Update `run_analyst_team()` to accept and forward `FredClient`

**Files:**
- Modify: `src/agents/analyst/mod.rs:76-99`

- [ ] **Step 1: Update `run_analyst_team()` signature**

  Add `fred: &FredClient` parameter after `finnhub: &FinnhubClient`:
  ```rust
  pub async fn run_analyst_team(
      handle: &CompletionModelHandle,
      finnhub: &FinnhubClient,
      fred: &FredClient,
      yfinance: &YFinanceClient,
      state: &mut TradingState,
      llm_config: &LlmConfig,
  ) -> Result<Vec<AgentTokenUsage>, TradingError> {
  ```

  Add `FredClient` import at the top of the file if not already present.

- [ ] **Step 2: Pass `fred` to `NewsAnalyst::new()`**

  In the `news_task` block (around line 127), change:
  ```rust
  let analyst = NewsAnalyst::new(
      handle.clone(),
      finnhub.clone(),
      symbol.clone(),
      target_date.clone(),
      llm_config,
      cached_news,
  );
  ```
  To:
  ```rust
  let analyst = NewsAnalyst::new(
      handle.clone(),
      finnhub.clone(),
      fred.clone(),
      symbol.clone(),
      target_date.clone(),
      llm_config,
      cached_news,
  );
  ```

- [ ] **Step 3: Commit**

  ```bash
  git add src/agents/analyst/mod.rs
  git commit -m "refactor: thread FredClient through run_analyst_team"
  ```

---

### Task 8: Update `NewsAnalystTask` workflow task

**Files:**
- Modify: `src/workflow/tasks/analyst.rs:224-270`

- [ ] **Step 1: Add `fred` field to `NewsAnalystTask`**

  Add after `finnhub: FinnhubClient`:
  ```rust
  fred: FredClient,
  ```

  Import `FredClient` at the top of the file.

- [ ] **Step 2: Update `NewsAnalystTask::new()`**

  Add `fred: FredClient` parameter:
  ```rust
  pub fn new(
      handle: CompletionModelHandle,
      finnhub: FinnhubClient,
      fred: FredClient,
      llm_config: LlmConfig,
  ) -> Arc<Self> {
      Arc::new(Self {
          handle,
          finnhub,
          fred,
          llm_config,
      })
  }
  ```

- [ ] **Step 3: Pass `fred` to `NewsAnalyst::new()` in the `run()` method**

  In the `run()` method (around line 264), change:
  ```rust
  let analyst = NewsAnalyst::new(
      self.handle.clone(),
      self.finnhub.clone(),
      state.asset_symbol.clone(),
      state.target_date.clone(),
      &self.llm_config,
      cached_news_opt,
  );
  ```
  To:
  ```rust
  let analyst = NewsAnalyst::new(
      self.handle.clone(),
      self.finnhub.clone(),
      self.fred.clone(),
      state.asset_symbol.clone(),
      state.target_date.clone(),
      &self.llm_config,
      cached_news_opt,
  );
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add src/workflow/tasks/analyst.rs
  git commit -m "refactor: thread FredClient through NewsAnalystTask"
  ```

---

### Task 9: Construct `FredClient` in the pipeline and pass it through

**Files:**
- Modify: `src/workflow/pipeline.rs:227-257`

- [ ] **Step 1: Add `FredClient` construction in `build_graph_impl()`**

  Import `FredClient` and `SharedRateLimiter::fred_from_config()` at the top. In `build_graph_impl()`, after the `finnhub` construction (find where `FinnhubClient` is built), add:
  ```rust
  let fred_limiter = SharedRateLimiter::fred_from_config(&config.rate_limits)
      .unwrap_or_else(|| SharedRateLimiter::disabled("fred"));
  let fred = FredClient::new(&config.api, fred_limiter)
      .expect("FredClient construction failed — is SCORPIO_FRED_API_KEY set?");
  ```

  **Note:** If `FredClient::new()` should be fallible at pipeline construction time, propagate the error using `?` instead of `expect()`. Check whether `build_graph_impl()` returns `Result` or panics on error — follow the existing pattern for `FinnhubClient`.

- [ ] **Step 2: Pass `fred` to `NewsAnalystTask::new()`**

  Change line 251 from:
  ```rust
  NewsAnalystTask::new(quick_handle.clone(), finnhub.clone(), config.llm.clone()),
  ```
  To:
  ```rust
  NewsAnalystTask::new(quick_handle.clone(), finnhub.clone(), fred.clone(), config.llm.clone()),
  ```

- [ ] **Step 3: If `TradingPipeline` stores `fred` as a field (needed for `build_graph()` etc.), add it**

  Follow the pattern used for `finnhub: FinnhubClient` on `TradingPipeline`. Add a `fred: FredClient` field, construct it in `TradingPipeline::new()`, and pass it to `build_graph_impl()`.

- [ ] **Step 4: Update any callers of `run_analyst_team()` if used outside the workflow**

  Search for direct callers of `run_analyst_team()` outside `workflow/`. If none exist (it's only used via the graph tasks), this step is a no-op.

- [ ] **Step 5: Verify full build compiles**

  Run: `cargo check`
  Expected: compiles with no errors

- [ ] **Step 6: Run all tests**

  Run: `cargo test`
  Expected: all tests pass (any test that previously constructed `NewsAnalystTask` or `NewsAnalyst` without `FredClient` will need updating — see Task 10)

- [ ] **Step 7: Commit**

  ```bash
  git add src/workflow/pipeline.rs
  git commit -m "feat: construct FredClient in pipeline, wire to NewsAnalystTask"
  ```

---

### Task 10: Fix broken tests from signature changes

**Files:**
- Modify: `src/workflow/tasks/tests.rs:438` (or wherever `NewsAnalystTask::new` is tested)
- Modify: `src/agents/analyst/news.rs` tests (if any construct `NewsAnalyst` directly)

- [ ] **Step 1: Find all test call sites**

  Run: `cargo test 2>&1` and collect compile errors related to `NewsAnalyst::new()` or `NewsAnalystTask::new()` missing the `fred` parameter.

- [ ] **Step 2: Add `FredClient::for_test()` at each call site**

  For each broken test, insert `FredClient::for_test()` as the new parameter. Example:
  ```rust
  NewsAnalystTask::new(handle, finnhub, FredClient::for_test(), llm_config)
  ```

- [ ] **Step 3: Run all tests**

  Run: `cargo test`
  Expected: all tests PASS

- [ ] **Step 4: Run clippy and fmt**

  Run: `cargo clippy -- -D warnings && cargo fmt -- --check`
  Expected: no warnings, no formatting issues

- [ ] **Step 5: Commit**

  ```bash
  git add -A
  git commit -m "test: fix signature changes from FredClient threading"
  ```

---

## Chunk 3: Cleanup

### Task 11: Remove dead Finnhub economic code and clean up

**Files:**
- Modify: `src/data/finnhub.rs` (verify no dead imports after removal)

- [ ] **Step 1: Verify no remaining references to Finnhub economic codes**

  Search the codebase for `MA-USA-656880`, `MA-USA-CPALTT01`, `INTEREST_RATE_CODE`, `INFLATION_CODE`.
  Expected: zero results.

- [ ] **Step 2: Verify `get_economic_indicators` only exists in `fred.rs`**

  Search the codebase for `get_economic_indicators`.
  Expected: only hits in `src/data/fred.rs`, `src/agents/analyst/news.rs`, and `src/data/mod.rs`.

- [ ] **Step 3: Run full test suite**

  Run: `cargo test`
  Expected: all pass

- [ ] **Step 4: Run clippy**

  Run: `cargo clippy -- -D warnings`
  Expected: no warnings

- [ ] **Step 5: Final commit**

  ```bash
  git add -A
  git commit -m "chore: clean up dead Finnhub economic code references"
  ```
