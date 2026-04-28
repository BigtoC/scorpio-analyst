//! Technical Analyst agent.
//!
//! Binds OHLCV and technical indicator tools to a quick-thinking LLM agent so
//! the model can fetch price history and compute indicators during inference,
//! then return a structured [`TechnicalData`] JSON object with an interpretive
//! summary.

use std::time::Instant;

use rig::tool::ToolDyn;

use crate::{
    agents::shared::agent_token_usage_from_completion,
    analysis_packs::RuntimePolicy,
    config::LlmConfig,
    constants::TECHNICAL_ANALYST_MAX_TURNS,
    data::{
        GetOhlcv, GetOptionsSnapshot, OhlcvToolContext, YFinanceClient, YFinanceOptionsProvider,
    },
    domain::Symbol,
    error::{RetryPolicy, TradingError},
    indicators::{
        CalculateAllIndicators, CalculateAtr, CalculateBollingerBands, CalculateIndicatorByName,
        CalculateMacd, CalculateRsi,
    },
    providers::factory::{CompletionModelHandle, build_agent_with_tools},
    state::{AgentTokenUsage, TechnicalData, TradingState},
};

use super::common::{
    analyst_runtime_config, render_analyst_system_prompt, run_analyst_inference,
    validate_summary_content,
};

/// Build the rendered system prompt for the Technical Analyst.
///
/// Reads the role's template from `RuntimePolicy.prompt_bundle.technical_analyst`
/// and delegates substitution and rule appending to the shared
/// [`render_analyst_system_prompt`] helper. Preflight's
/// `validate_active_pack_completeness` gate ensures the slot is non-empty
/// before any analyst task runs.
pub(crate) fn build_technical_system_prompt(
    symbol: &str,
    target_date: &str,
    policy: &RuntimePolicy,
) -> String {
    render_analyst_system_prompt(
        policy.prompt_bundle.technical_analyst.as_ref(),
        symbol,
        target_date,
        policy,
    )
}

/// Number of calendar days of OHLCV history to request.
const OHLCV_LOOKBACK_DAYS: i64 = 365;

/// The Technical Analyst agent.
///
/// Binds OHLCV and indicator tools to the LLM so it can fetch price history
/// and compute indicators during inference, then write a decision-relevant
/// [`TechnicalData`] summary.
pub struct TechnicalAnalyst {
    handle: CompletionModelHandle,
    yfinance: YFinanceClient,
    symbol: String,
    target_date: String,
    system_prompt: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
    // Stored for test assertions; not read by production code.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) typed_symbol: Symbol,
}

impl TechnicalAnalyst {
    /// Construct a new `TechnicalAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (`QuickThinking` tier).
    /// - `yfinance` – Yahoo Finance client for OHLCV fetching.
    /// - `state` – current trading state, including any active runtime policy.
    /// - `policy` – resolved runtime policy for the active analysis pack.
    /// - `llm_config` – LLM configuration, used for timeout.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::SchemaViolation`] if `state.symbol` is `None`,
    /// which indicates the symbol was not canonicalized by [`TradingState::new`].
    pub fn new(
        handle: CompletionModelHandle,
        yfinance: YFinanceClient,
        state: &TradingState,
        policy: &RuntimePolicy,
        llm_config: &LlmConfig,
    ) -> Result<Self, TradingError> {
        let typed_symbol = state.symbol.clone().ok_or_else(|| TradingError::SchemaViolation {
            message: "TechnicalAnalyst::new called with state.symbol = None; expected canonicalized symbol from TradingState::new".to_owned(),
        })?;

        let runtime = analyst_runtime_config(&state.asset_symbol, &state.target_date, llm_config);
        let system_prompt =
            build_technical_system_prompt(&runtime.symbol, &runtime.target_date, policy);

        Ok(Self {
            handle,
            yfinance,
            symbol: runtime.symbol,
            target_date: runtime.target_date,
            system_prompt,
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
            typed_symbol,
        })
    }

    /// Run the analyst: bind OHLCV + indicator tools to the LLM, prompt it, parse output.
    ///
    /// # Errors
    ///
    /// - [`TradingError::SchemaViolation`] when `target_date` is not a valid ISO date or LLM output is malformed.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    pub async fn run(&self) -> Result<(TechnicalData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let start_date = derive_start_date(&self.target_date, OHLCV_LOOKBACK_DAYS)?;
        let ohlcv_context = OhlcvToolContext::new();

        let options_provider = YFinanceOptionsProvider::new(self.yfinance.clone());
        let tools: Vec<Box<dyn ToolDyn>> = vec![
            Box::new(GetOhlcv::scoped(
                self.yfinance.clone(),
                self.symbol.clone(),
                start_date.clone(),
                self.target_date.clone(),
                ohlcv_context.clone(),
            )),
            Box::new(CalculateAllIndicators::new(ohlcv_context.clone())),
            Box::new(CalculateRsi::new(ohlcv_context.clone())),
            Box::new(CalculateMacd::new(ohlcv_context.clone())),
            Box::new(CalculateAtr::new(ohlcv_context.clone())),
            Box::new(CalculateBollingerBands::new(ohlcv_context.clone())),
            Box::new(CalculateIndicatorByName::new(ohlcv_context)),
            Box::new(GetOptionsSnapshot::scoped(
                options_provider,
                self.symbol.clone(),
                self.target_date.clone(),
            )),
        ];

        let agent = build_agent_with_tools(&self.handle, &self.system_prompt, tools);

        let prompt = format!(
            "Fetch OHLCV data for {} from {} to {} using get_ohlcv, compute indicators with \
             calculate_all_indicators, then produce a TechnicalData JSON object.",
            self.symbol, start_date, self.target_date
        );

        let outcome = run_analyst_inference(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            TECHNICAL_ANALYST_MAX_TURNS,
            parse_technical,
            validate_technical,
        )
        .await?;

        let usage = agent_token_usage_from_completion(
            "Technical Analyst",
            self.handle.model_id(),
            outcome.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        Ok((outcome.output, usage))
    }
}

fn validate_technical(data: &TechnicalData) -> Result<(), TradingError> {
    if data.summary.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "TechnicalAnalyst: summary must not be empty".to_owned(),
        });
    }
    validate_summary_content("TechnicalAnalyst", &data.summary)?;
    if let Some(rsi) = data.rsi
        && !(0.0..=100.0).contains(&rsi)
    {
        return Err(TradingError::SchemaViolation {
            message: format!("TechnicalAnalyst: RSI {rsi} must be within [0, 100]"),
        });
    }
    Ok(())
}

/// Deserialize a JSON string into [`TechnicalData`], mapping errors to
/// [`TradingError::SchemaViolation`].
///
/// Exposed for use as the `parse` hook in `run_analyst_inference`.
pub(crate) fn parse_technical(json_str: &str) -> Result<TechnicalData, TradingError> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| TradingError::SchemaViolation {
            message: format!("TechnicalAnalyst: failed to parse LLM output: {e}"),
        })?;

    if value.get("macd").is_some_and(|macd| macd.is_number()) {
        return Err(TradingError::SchemaViolation {
            message: "TechnicalAnalyst: failed to parse LLM output: field `macd` must be an object with `macd_line`, `signal_line`, and `histogram`, or null".to_owned(),
        });
    }

    serde_json::from_value(value).map_err(|e| TradingError::SchemaViolation {
        message: format!("TechnicalAnalyst: failed to parse LLM output: {e}"),
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Compute the OHLCV start date by subtracting `days` calendar days from
/// `end_date` (ISO 8601 string `"YYYY-MM-DD"`).
fn derive_start_date(end_date: &str, days: i64) -> Result<String, TradingError> {
    use chrono::NaiveDate;
    let end = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").map_err(|e| {
        TradingError::SchemaViolation {
            message: format!("TechnicalAnalyst: invalid target_date {end_date:?}: {e}"),
        }
    })?;
    let start = end
        .checked_sub_signed(chrono::Duration::days(days))
        .ok_or_else(|| TradingError::SchemaViolation {
            message: "TechnicalAnalyst: date arithmetic overflow".to_owned(),
        })?;
    Ok(start.format("%Y-%m-%d").to_string())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MacdValues, TechnicalData};

    /// Parse and validate a JSON string — combines `parse_technical` + `validate_technical`
    /// for test convenience. Tests that need only structural parsing can call `parse_technical`
    /// directly; tests that also exercise the semantic validation layer call this helper.
    fn parse_and_validate(json: &str) -> Result<TechnicalData, TradingError> {
        parse_technical(json).and_then(|data| validate_technical(&data).map(|()| data))
    }

    // ── Task 4.4: Correct TechnicalData extraction ────────────────────────

    #[test]
    fn parses_valid_technical_json_with_all_fields() {
        let json = r#"{
            "rsi": 58.3,
            "macd": {
                "macd_line": 0.42,
                "signal_line": 0.35,
                "histogram": 0.07
            },
            "atr": 2.15,
            "sma_20": 175.30,
            "sma_50": 172.10,
            "ema_12": 176.50,
            "ema_26": 174.20,
            "bollinger_upper": 180.00,
            "bollinger_lower": 170.00,
            "support_level": 168.00,
            "resistance_level": 182.00,
            "volume_avg": 55000000.0,
            "summary": "Bullish momentum; RSI moderate, price above SMA50."
        }"#;

        let data = parse_and_validate(json).expect("should parse");
        assert!((data.rsi.unwrap() - 58.3).abs() < 1e-9);
        let macd = data.macd.as_ref().unwrap();
        assert!((macd.macd_line - 0.42).abs() < 1e-9);
        assert!((macd.signal_line - 0.35).abs() < 1e-9);
        assert!((macd.histogram - 0.07).abs() < 1e-9);
        assert!((data.atr.unwrap() - 2.15).abs() < 1e-9);
        assert!((data.sma_50.unwrap() - 172.10).abs() < 1e-9);
        assert!((data.support_level.unwrap() - 168.00).abs() < 1e-9);
        assert!((data.resistance_level.unwrap() - 182.00).abs() < 1e-9);
        assert!(!data.summary.is_empty());
    }

    // ── Task 4.5: Prompt-compatible indicator names ───────────────────────

    fn baseline_technical_prompt() -> &'static str {
        crate::testing::baseline_pack_prompt_for_role(crate::workflow::Role::TechnicalAnalyst)
    }

    #[test]
    fn system_prompt_mentions_prompt_compatible_indicator_names() {
        // Drift-detection guard against the canonical runtime source — the
        // baseline pack's `PromptBundle.technical_analyst` slot.
        let prompt = baseline_technical_prompt();
        let names = ["rsi", "macd", "atr", "boll", "boll_ub", "boll_lb", "vwma"];
        for name in &names {
            assert!(
                prompt.contains(name),
                "baseline technical prompt should mention indicator name: {name}"
            );
        }
    }

    #[test]
    fn system_prompt_warns_that_options_snapshot_omits_skew() {
        let prompt = baseline_technical_prompt();
        let prompt_lower = prompt.to_lowercase();

        assert!(
            prompt_lower.contains("skew"),
            "baseline technical prompt must mention the missing skew context: {prompt}"
        );
        assert!(
            prompt_lower.contains("directional vol"),
            "baseline technical prompt must forbid directional vol calls without skew context: {prompt}"
        );
    }

    // ── Task 4.6: Partial results with insufficient OHLCV data ───────────

    #[test]
    fn parses_technical_with_all_null_fields() {
        let json = r#"{
            "rsi": null,
            "macd": null,
            "atr": null,
            "sma_20": null,
            "sma_50": null,
            "ema_12": null,
            "ema_26": null,
            "bollinger_upper": null,
            "bollinger_lower": null,
            "support_level": null,
            "resistance_level": null,
            "volume_avg": null,
            "summary": "Insufficient OHLCV history for indicator computation."
        }"#;

        let data = parse_and_validate(json).expect("should parse all-null");
        assert!(data.rsi.is_none());
        assert!(data.macd.is_none());
        assert!(data.atr.is_none());
        assert!(!data.summary.is_empty());
    }

    #[test]
    fn derive_start_date_subtracts_correct_days() {
        let result = derive_start_date("2026-03-14", 365).unwrap();
        assert_eq!(result, "2025-03-14");
    }

    #[test]
    fn derive_start_date_invalid_format_returns_error() {
        let result = derive_start_date("not-a-date", 100);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── AgentTokenUsage recording ─────────────────────────────────────────

    #[test]
    fn agent_token_usage_fields() {
        let usage = AgentTokenUsage {
            agent_name: "Technical Analyst".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 300,
            rate_limit_wait_ms: 0,
        };
        assert_eq!(usage.agent_name, "Technical Analyst");
        assert_eq!(usage.model_id, "gpt-4o-mini");
    }

    // ── SchemaViolation on malformed JSON ─────────────────────────────────

    #[test]
    fn malformed_json_returns_schema_violation() {
        let result = parse_technical("not json");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn json_missing_summary_returns_schema_violation() {
        // `summary` is required
        let result = parse_technical(r#"{"rsi": 50.0}"#);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn rsi_above_100_returns_schema_violation() {
        let json = r#"{
            "rsi": 101.0, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "invalid rsi"
        }"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn rsi_below_zero_returns_schema_violation() {
        let json = r#"{
            "rsi": -1.0, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "invalid rsi"
        }"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn whitespace_only_summary_returns_schema_violation() {
        let json = r#"{
            "rsi": null, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "   "
        }"#;
        let result = parse_and_validate(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Struct round-trip ─────────────────────────────────────────────────

    #[test]
    fn technical_data_round_trips_through_json() {
        let original = TechnicalData {
            rsi: Some(55.0),
            macd: Some(MacdValues {
                macd_line: 0.1,
                signal_line: 0.05,
                histogram: 0.05,
            }),
            atr: Some(1.5),
            sma_20: Some(150.0),
            sma_50: None,
            ema_12: Some(151.0),
            ema_26: Some(149.0),
            bollinger_upper: Some(160.0),
            bollinger_lower: Some(140.0),
            support_level: None,
            resistance_level: None,
            volume_avg: Some(500_000.0),
            summary: "Neutral trend.".to_owned(),
            options_summary: None,
            options_context: None,
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: TechnicalData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }

    // TC-12: RSI at exact lower boundary 0.0 is valid
    #[test]
    fn rsi_at_zero_boundary_is_valid() {
        let json = r#"{
            "rsi": 0.0, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "boundary"
        }"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "RSI = 0.0 must be accepted (inclusive lower bound)"
        );
    }

    // TC-12: RSI at exact upper boundary 100.0 is valid
    #[test]
    fn rsi_at_100_boundary_is_valid() {
        let json = r#"{
            "rsi": 100.0, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "boundary"
        }"#;
        assert!(
            parse_and_validate(json).is_ok(),
            "RSI = 100.0 must be accepted (inclusive upper bound)"
        );
    }

    // TC-13: derive_start_date date arithmetic — the checked_sub_signed overflow
    // branch is only reachable for dates near NaiveDate::MIN, which cannot be
    // represented as a parseable "%Y-%m-%d" string (chrono requires 4-digit years
    // for the %Y specifier, and NaiveDate::MIN is "-262143-01-01").
    //
    // This test documents that "0001-01-01" — the earliest parseable year — does
    // NOT overflow when subtracting 365 days, confirming the branch is dead code
    // for any valid real-world ISO 8601 input.
    #[test]
    fn derive_start_date_earliest_parseable_year_does_not_overflow() {
        // Year 1 minus 365 days lands in year 0000 (proleptic Gregorian),
        // which is representable in chrono and therefore does not overflow.
        let result = derive_start_date("0001-01-01", 365);
        assert!(
            result.is_ok(),
            "the earliest parseable year should not overflow; got: {result:?}"
        );
    }

    // ── Task 6: Migrate to shared inference helper ────────────────────────

    #[test]
    fn technical_prompt_limits_get_ohlcv_to_one_call() {
        let prompt = baseline_technical_prompt();
        assert!(
            prompt.contains("get_ohlcv"),
            "baseline technical prompt must contain 'get_ohlcv'"
        );
        assert!(
            prompt.contains("called at most once"),
            "baseline technical prompt must contain 'called at most once'"
        );
        assert!(
            prompt.contains("indicator tools"),
            "baseline technical prompt must contain 'indicator tools'"
        );
    }

    #[test]
    fn technical_prompt_requires_exactly_one_json_object_response() {
        let prompt = baseline_technical_prompt();
        assert!(
            prompt.contains("exactly one JSON object"),
            "baseline technical prompt must contain 'exactly one JSON object'"
        );
        assert!(
            prompt.contains("no prose"),
            "baseline technical prompt must contain 'no prose'"
        );
        assert!(
            prompt.contains("no markdown fences"),
            "baseline technical prompt must contain 'no markdown fences'"
        );
    }

    #[test]
    fn technical_prompt_describes_macd_object_shape() {
        let prompt = baseline_technical_prompt();
        for field in ["macd_line", "signal_line", "histogram"] {
            assert!(
                prompt.contains(field),
                "baseline technical prompt must describe MACD field: {field}"
            );
        }
    }

    #[tokio::test]
    async fn technical_run_uses_shared_inference_helper_for_openrouter() {
        use super::super::common::run_analyst_inference;
        use crate::providers::ProviderId;
        use crate::providers::factory::agent_test_support;
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let valid_json = r#"{
            "rsi": 55.0,
            "macd": null,
            "atr": null,
            "sma_20": null,
            "sma_50": null,
            "ema_12": null,
            "ema_26": null,
            "bollinger_upper": null,
            "bollinger_lower": null,
            "support_level": null,
            "resistance_level": null,
            "volume_avg": null,
            "summary": "Moderate bullish momentum."
        }"#;

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new(
            valid_json,
            Usage {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        ));

        let outcome = run_analyst_inference::<TechnicalData, _, _>(
            &agent,
            "analyse AAPL technical",
            std::time::Duration::from_millis(100),
            &crate::error::RetryPolicy {
                max_retries: 0,
                base_delay: std::time::Duration::from_millis(1),
            },
            1,
            super::parse_technical,
            super::validate_technical,
        )
        .await
        .expect("inference should succeed");

        let _ = outcome.output;

        assert_eq!(agent_test_support::typed_attempts(&agent), 0);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }

    #[test]
    fn technical_data_missing_options_summary_defaults_to_none() {
        // Backward-compat: pre-options-snapshot `TechnicalData` payloads omit
        // the `options_summary` field. Deserialization must default it to
        // None instead of failing.
        let json = r#"{
            "rsi": 55.0,
            "macd": null,
            "atr": null,
            "sma_20": null,
            "sma_50": null,
            "ema_12": null,
            "ema_26": null,
            "bollinger_upper": null,
            "bollinger_lower": null,
            "support_level": null,
            "resistance_level": null,
            "volume_avg": null,
            "summary": "Legacy snapshot without options."
        }"#;
        let data = parse_technical(json)
            .expect("legacy snapshot without options_summary must deserialize");
        assert!(
            data.options_summary.is_none(),
            "missing options_summary field should default to None"
        );
    }

    #[test]
    fn technical_data_missing_options_context_defaults_to_none() {
        let json = r#"{
            "rsi": 55.0,
            "macd": null,
            "atr": null,
            "sma_20": null,
            "sma_50": null,
            "ema_12": null,
            "ema_26": null,
            "bollinger_upper": null,
            "bollinger_lower": null,
            "support_level": null,
            "resistance_level": null,
            "volume_avg": null,
            "summary": "legacy technical payload",
            "options_summary": null
        }"#;

        let data: TechnicalData =
            serde_json::from_str(json).expect("legacy payload should deserialize");
        assert!(data.options_context.is_none());
    }

    #[test]
    fn scalar_macd_value_returns_schema_violation_with_macd_shape_hint() {
        let json = r#"{
            "rsi": 43.2,
            "macd": -2.87,
            "atr": 4.12,
            "sma_20": 191.4,
            "sma_50": 198.2,
            "ema_12": 193.1,
            "ema_26": 195.97,
            "bollinger_upper": 205.0,
            "bollinger_lower": 188.0,
            "support_level": 190.0,
            "resistance_level": 201.0,
            "volume_avg": 64000000.0,
            "summary": "Momentum is weakening and MACD is negative."
        }"#;

        let err = parse_technical(json).expect_err("scalar MACD should not silently parse");

        match err {
            TradingError::SchemaViolation { message } => {
                assert!(message.contains("TechnicalAnalyst: failed to parse LLM output"));
                assert!(message.contains("macd"));
                assert!(
                    message.contains("macd_line")
                        && message.contains("signal_line")
                        && message.contains("histogram"),
                    "error should explain the required MACD object shape: {message}"
                );
            }
            other => panic!("expected SchemaViolation, got: {other:?}"),
        }
    }

    // ── Task chunk1: Rendered-prompt evidence-discipline hardening ─────────

    #[test]
    fn technical_rendered_prompt_includes_evidence_discipline_rules() {
        use crate::analysis_packs::resolve_runtime_policy;

        let policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        let prompt = build_technical_system_prompt("AAPL", "2026-01-01", &policy);

        for phrase in [
            "Prefer authoritative runtime evidence",
            "When evidence is sparse or missing",
            "Separate observed facts (tool output) from interpretation",
            "Do not infer estimates",
            "sparse or missing",
            "Separate observed facts",
        ] {
            assert!(
                prompt.contains(phrase),
                "rendered prompt must contain runtime-contract phrase {phrase:?}"
            );
        }
    }

    // ── Task 7: GetOptionsSnapshot wiring ─────────────────────────────────

    #[test]
    fn parses_technical_with_options_summary() {
        let json = r#"{
            "rsi": 52.0,
            "macd": null,
            "atr": null,
            "sma_20": null,
            "sma_50": null,
            "ema_12": null,
            "ema_26": null,
            "bollinger_upper": null,
            "bollinger_lower": null,
            "support_level": null,
            "resistance_level": null,
            "volume_avg": null,
            "summary": "Moderate bullish trend.",
            "options_summary": "{\"kind\":\"snapshot\",\"spot_price\":150.0,\"atm_iv\":0.25}"
        }"#;
        let data = parse_technical(json).expect("should parse with options_summary");
        assert!(
            data.options_summary.is_some(),
            "options_summary should be Some when present in JSON"
        );
        assert!(
            data.options_summary
                .as_deref()
                .unwrap()
                .contains("snapshot"),
            "options_summary should contain the snapshot kind"
        );
    }

    #[tokio::test]
    async fn technical_tool_renders_options_outcome_variant_with_reason() {
        use crate::data::yfinance::options::OptionsSnapshotArgs;
        use crate::data::{
            GetOptionsSnapshot, StubbedFinancialResponses, YFinanceClient, YFinanceOptionsProvider,
        };

        // Stub with a past date so HistoricalRun is returned.
        let client = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            ohlcv: Some(vec![]),
            option_expirations: Some(vec![1_000_000]),
            ..StubbedFinancialResponses::default()
        });
        let provider = YFinanceOptionsProvider::new(client);
        let tool = GetOptionsSnapshot::scoped(provider, "AAPL", "2020-01-01");

        let result: serde_json::Value = rig::tool::Tool::call(
            &tool,
            OptionsSnapshotArgs {
                symbol: "AAPL".to_owned(),
                target_date: "2020-01-01".to_owned(),
            },
        )
        .await
        .expect("tool call should succeed");

        // The "2020-01-01" date is in the past → HistoricalRun variant.
        assert_eq!(
            result.get("kind").and_then(|v| v.as_str()),
            Some("historical_run"),
            "kind should be historical_run for a past date"
        );
        assert!(
            result.get("reason").is_some(),
            "reason must be injected for non-Snapshot variants"
        );
        let reason = result["reason"].as_str().unwrap();
        assert!(
            reason.contains("target_date") || reason.contains("US/Eastern"),
            "reason should mention the temporal constraint, got: {reason}"
        );
    }

    #[test]
    fn technical_analyst_new_stays_infallible_for_canonical_equity_symbol() {
        use crate::analysis_packs::resolve_runtime_policy;

        let policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        let llm_config = crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        };
        let state = TradingState::new("AAPL", "2026-01-01");
        // state.symbol is set by TradingState::new for valid ticker strings
        assert!(
            state.symbol.is_some(),
            "TradingState::new must populate state.symbol for a canonical equity symbol"
        );

        let handle = crate::providers::factory::CompletionModelHandle::for_test();
        let yfinance = crate::data::YFinanceClient::default();

        let result = TechnicalAnalyst::new(handle, yfinance, &state, &policy, &llm_config);
        assert!(
            result.is_ok(),
            "TechnicalAnalyst::new should succeed for a canonical equity symbol, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn technical_analyst_new_rejects_missing_typed_symbol() {
        use crate::analysis_packs::resolve_runtime_policy;

        let policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        let llm_config = crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        };
        let mut state = TradingState::new("AAPL", "2026-01-01");
        state.symbol = None;

        let handle = crate::providers::factory::CompletionModelHandle::for_test();
        let yfinance = crate::data::YFinanceClient::default();

        match TechnicalAnalyst::new(handle, yfinance, &state, &policy, &llm_config) {
            Ok(_) => panic!("missing typed symbol must be rejected"),
            Err(TradingError::SchemaViolation { message }) => {
                assert!(message.contains("state.symbol = None"));
            }
            Err(other) => panic!("expected SchemaViolation, got: {other:?}"),
        }
    }

    #[test]
    fn technical_analyst_new_propagates_symbol_from_state_without_reparsing() {
        use crate::analysis_packs::resolve_runtime_policy;
        use crate::domain::{Symbol, Ticker};

        let policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        let llm_config = crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        };
        let state = TradingState::new("MSFT", "2026-01-01");
        let expected_symbol = Symbol::Equity(Ticker::parse("MSFT").unwrap());

        let handle = crate::providers::factory::CompletionModelHandle::for_test();
        let yfinance = crate::data::YFinanceClient::default();

        let analyst = TechnicalAnalyst::new(handle, yfinance, &state, &policy, &llm_config)
            .expect("test fixture must canonicalize symbol");

        assert_eq!(
            analyst.typed_symbol, expected_symbol,
            "typed_symbol must be the Symbol from state.symbol, not re-parsed"
        );
    }

    #[test]
    fn technical_rendered_prompt_prefers_runtime_policy_prompt_bundle() {
        use crate::analysis_packs::resolve_runtime_policy;

        let mut policy =
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve");
        policy.analysis_emphasis = "stress momentum and key levels".to_owned();
        policy.prompt_bundle.technical_analyst =
            "Pack technical prompt for {ticker} at {current_date}. Emphasis: {analysis_emphasis}."
                .into();

        let prompt = build_technical_system_prompt("AAPL", "2026-01-01", &policy);

        assert!(
            prompt.contains(
                "Pack technical prompt for AAPL at 2026-01-01. Emphasis: stress momentum and key levels."
            ),
            "runtime-policy prompt bundle should drive the technical prompt body: {prompt}"
        );
    }
}
