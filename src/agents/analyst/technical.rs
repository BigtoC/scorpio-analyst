//! Technical Analyst agent.
//!
//! Binds OHLCV and technical indicator tools to a quick-thinking LLM agent so
//! the model can fetch price history and compute indicators during inference,
//! then return a structured [`TechnicalData`] JSON object with an interpretive
//! summary.

use std::time::Instant;

use rig::tool::ToolDyn;

use crate::{
    config::LlmConfig,
    data::{GetOhlcv, OhlcvToolContext, YFinanceClient},
    error::{RetryPolicy, TradingError},
    indicators::{
        CalculateAllIndicators, CalculateAtr, CalculateBollingerBands, CalculateIndicatorByName,
        CalculateMacd, CalculateRsi,
    },
    providers::factory::{CompletionModelHandle, build_agent_with_tools},
    state::{AgentTokenUsage, TechnicalData},
};

use super::common::{
    analyst_runtime_config, run_analyst_inference, usage_from_response, validate_summary_content,
};

const MAX_TOOL_TURNS: usize = 10;

/// System prompt for the Technical Analyst, adapted from `docs/prompts.md`.
const TECHNICAL_SYSTEM_PROMPT: &str = "\
You are the Technical Analyst for {ticker} as of {current_date}.
Your job is to interpret tool-computed technical signals and return a `TechnicalData` JSON object.

Use only the technical indicator tools bound for the run. Current runtime tools may include:
- `get_ohlcv` — call get_ohlcv called at most once per run
- `calculate_all_indicators`
- `calculate_rsi`
- `calculate_macd`
- `calculate_atr`
- `calculate_bollinger_bands`
- `calculate_indicator_by_name`

Important constraints:
- Do not paste raw OHLCV candles into your response.
- Prefer `calculate_all_indicators` when it is available.
- If the runtime exposes only named-indicator selection, use the exact supported indicator names:
  `close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`, `macdh`, `rsi`, `boll`, `boll_ub`, `boll_lb`, \
  `atr`, `vwma`.

Populate only these schema fields:
- `rsi`
- `macd` — either `null` or an object with `macd_line`, `signal_line`, and `histogram`
- `atr`
- `sma_20`
- `sma_50`
- `ema_12`
- `ema_26`
- `bollinger_upper`
- `bollinger_lower`
- `support_level`
- `resistance_level`
- `volume_avg`
- `summary`

Instructions:
1. Focus on trend, momentum, volatility, and key levels instead of dumping every reading.
2. If an indicator cannot be computed because of limited history, preserve that absence with `null` rather than \
   guessing.
3. Interpret tool output; do not claim you calculated indicators manually.
4. The `macd` output field is not a scalar named-indicator value. When present, set it to an object with \
   `macd_line`, `signal_line`, and `histogram`. If you cannot provide all three, use `null`.
5. Some named indicators may exist for reasoning but not as dedicated output fields. For example, if `close_200_sma`, \
   `close_10_ema`, or a scalar named-indicator value like `macd` is available, use it for reasoning only unless you can \
   populate the full `macd` object without inventing values.
6. Keep `summary` short and useful for the Trader and risk agents.
7. Return exactly one JSON object required by `TechnicalData`. No prose, no markdown fences — output exactly one JSON object, no prose, no markdown fences.

Do not include any trade recommendation, target price, or final transaction proposal.";

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
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl TechnicalAnalyst {
    /// Construct a new `TechnicalAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (`QuickThinking` tier).
    /// - `yfinance` – Yahoo Finance client for OHLCV fetching.
    /// - `symbol` – asset ticker symbol.
    /// - `target_date` – analysis date string (ISO 8601, e.g. `"2026-03-14"`).
    /// - `llm_config` – LLM configuration, used for timeout.
    pub fn new(
        handle: CompletionModelHandle,
        yfinance: YFinanceClient,
        symbol: impl Into<String>,
        target_date: impl Into<String>,
        llm_config: &LlmConfig,
    ) -> Self {
        let runtime = analyst_runtime_config(symbol, target_date, llm_config);

        Self {
            handle,
            yfinance,
            symbol: runtime.symbol,
            target_date: runtime.target_date,
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
        }
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
        ];

        let system_prompt = TECHNICAL_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent_with_tools(&self.handle, &system_prompt, tools);

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
            MAX_TOOL_TURNS,
            parse_technical,
            validate_technical,
        )
        .await?;

        let usage = usage_from_response(
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
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        TradingError::SchemaViolation {
            message: format!("TechnicalAnalyst: failed to parse LLM output: {e}"),
        }
    })?;

    if value
        .get("macd")
        .is_some_and(|macd| macd.is_number())
    {
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

    #[test]
    fn system_prompt_mentions_prompt_compatible_indicator_names() {
        let names = ["rsi", "macd", "atr", "boll", "boll_ub", "boll_lb", "vwma"];
        for name in &names {
            assert!(
                TECHNICAL_SYSTEM_PROMPT.contains(name),
                "system prompt should mention indicator name: {name}"
            );
        }
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
    fn extra_fields_in_json_are_rejected() {
        let json = r#"{
            "rsi": null, "macd": null, "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "ok",
            "unexpected_field": "should fail"
        }"#;
        let result = parse_technical(json);
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
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("get_ohlcv"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'get_ohlcv'"
        );
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("called at most once"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'called at most once'"
        );
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("indicator tools"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'indicator tools'"
        );
    }

    #[test]
    fn technical_prompt_requires_exactly_one_json_object_response() {
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("exactly one JSON object"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'exactly one JSON object'"
        );
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("no prose"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'no prose'"
        );
        assert!(
            TECHNICAL_SYSTEM_PROMPT.contains("no markdown fences"),
            "TECHNICAL_SYSTEM_PROMPT must contain 'no markdown fences'"
        );
    }

    #[test]
    fn technical_prompt_describes_macd_object_shape() {
        for field in ["macd_line", "signal_line", "histogram"] {
            assert!(
                TECHNICAL_SYSTEM_PROMPT.contains(field),
                "TECHNICAL_SYSTEM_PROMPT must describe MACD field: {field}"
            );
        }
    }

    #[test]
    fn parse_technical_rejects_unknown_fields() {
        let result = super::parse_technical(r#"{"unknown_field": 1}"#);
        assert!(
            matches!(result, Err(TradingError::SchemaViolation { .. })),
            "parse_technical should return SchemaViolation for unknown fields"
        );
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

    // TC-16: MacdValues rejects extra fields (deny_unknown_fields)
    #[test]
    fn macd_values_extra_fields_rejected() {
        let json = r#"{
            "rsi": null,
            "macd": {"macd_line": 0.1, "signal_line": 0.05, "histogram": 0.05, "extra": "bad"},
            "atr": null, "sma_20": null, "sma_50": null,
            "ema_12": null, "ema_26": null, "bollinger_upper": null, "bollinger_lower": null,
            "support_level": null, "resistance_level": null, "volume_avg": null,
            "summary": "should fail"
        }"#;
        let result = parse_technical(json);
        assert!(
            result.is_err(),
            "extra field inside MacdValues should be rejected"
        );
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
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
}
