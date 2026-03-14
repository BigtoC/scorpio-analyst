//! Technical Analyst agent.
//!
//! Fetches OHLCV bars from Yahoo Finance, computes all technical indicators
//! in Rust via [`calculate_all_indicators`], formats the results as context,
//! and passes them to a quick-thinking LLM that returns a structured
//! [`TechnicalData`] JSON object with an interpretive summary.

use std::time::Instant;

use crate::{
    config::LlmConfig,
    data::YFinanceClient,
    error::{RetryPolicy, TradingError},
    indicators::calculate_all_indicators,
    providers::factory::{CompletionModelHandle, build_agent, prompt_with_retry},
    state::{AgentTokenUsage, TechnicalData},
};

/// System prompt for the Technical Analyst, adapted from `docs/prompts.md`.
const TECHNICAL_SYSTEM_PROMPT: &str = "\
You are the Technical Analyst for {ticker} as of {current_date}.
Your job is to interpret precomputed or tool-computed technical signals and return a `TechnicalData` JSON object.

Use only the technical tools bound for the run. Current runtime tools may include:
- `get_ohlcv`
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
- `macd`
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
4. Some named indicators may exist for reasoning but not as dedicated output fields. For example, if `close_200_sma` or \
   `close_10_ema` is available, use it for reasoning only and fold the insight into `summary` rather than inventing new \
   JSON keys.
5. Keep `summary` short and useful for the Trader and risk agents.
6. Return ONLY the single JSON object required by `TechnicalData`.

Do not include any trade recommendation, target price, or final transaction proposal.";

/// Number of calendar days of OHLCV history to request.
const OHLCV_LOOKBACK_DAYS: i64 = 365;

/// The Technical Analyst agent.
///
/// Computes all technical indicators from OHLCV data in Rust, then invokes an
/// LLM to interpret the indicators and write a decision-relevant summary.
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
        Self {
            handle,
            yfinance,
            symbol: symbol.into(),
            target_date: target_date.into(),
            timeout: std::time::Duration::from_secs(llm_config.agent_timeout_secs),
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Run the analyst: fetch OHLCV, compute indicators, prompt LLM, parse output.
    ///
    /// # Errors
    ///
    /// - [`TradingError::AnalystError`] when OHLCV fetching fails.
    /// - [`TradingError::SchemaViolation`] when indicators cannot be computed or LLM output is malformed.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    pub async fn run(&self) -> Result<(TechnicalData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        // ── 1. Derive OHLCV date range ────────────────────────────────────
        let end_date = &self.target_date;
        let start_date = derive_start_date(end_date, OHLCV_LOOKBACK_DAYS)?;

        // ── 2. Fetch OHLCV bars ───────────────────────────────────────────
        let candles = self
            .yfinance
            .get_ohlcv(&self.symbol, &start_date, end_date)
            .await
            .map_err(|e| TradingError::AnalystError {
                agent: "technical".to_owned(),
                message: e.to_string(),
            })?;

        // ── 3. Compute indicators in Rust ─────────────────────────────────
        // If no candles were returned (e.g. holiday/weekend), return partial TechnicalData.
        let computed = if candles.is_empty() {
            TechnicalData {
                rsi: None,
                macd: None,
                atr: None,
                sma_20: None,
                sma_50: None,
                ema_12: None,
                ema_26: None,
                bollinger_upper: None,
                bollinger_lower: None,
                support_level: None,
                resistance_level: None,
                volume_avg: None,
                summary: "No OHLCV data available for the requested period.".to_owned(),
            }
        } else {
            calculate_all_indicators(&candles)?
        };

        // ── 4. Format indicator context (no raw OHLCV) ────────────────────
        let context = format_indicator_context(&computed);

        // ── 5. Build agent and invoke LLM ─────────────────────────────────
        let system_prompt = TECHNICAL_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent(&self.handle, &system_prompt);

        let prompt = format!(
            "Using the precomputed technical indicators below for {} as of {}, \
             interpret the signals and produce a `TechnicalData` JSON object.\n\n{}",
            self.symbol, self.target_date, context
        );

        let raw = prompt_with_retry(&agent, &prompt, self.timeout, &self.retry_policy).await?;

        // ── 6. Parse structured output ────────────────────────────────────
        let data: TechnicalData =
            serde_json::from_str(raw.trim()).map_err(|e| TradingError::SchemaViolation {
                message: format!("TechnicalAnalyst: failed to parse LLM output: {e}"),
            })?;

        // ── 7. Record token usage ─────────────────────────────────────────
        let latency_ms = started_at.elapsed().as_millis() as u64;
        let usage = AgentTokenUsage {
            agent_name: "technical".to_owned(),
            model_id: self.handle.model_id().to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms,
        };

        Ok((data, usage))
    }
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

/// Format a computed [`TechnicalData`] as human-readable context lines.
///
/// Deliberately avoids raw OHLCV candles — only indicator values are included.
fn format_indicator_context(td: &TechnicalData) -> String {
    let mut lines = vec!["## Precomputed Technical Indicators".to_owned()];

    lines.push(format!("RSI (14): {}", fmt_opt(td.rsi)));
    if let Some(ref m) = td.macd {
        lines.push(format!(
            "MACD: line={:.4}, signal={:.4}, histogram={:.4}",
            m.macd_line, m.signal_line, m.histogram
        ));
    } else {
        lines.push("MACD: null".to_owned());
    }
    lines.push(format!("ATR (14): {}", fmt_opt(td.atr)));
    lines.push(format!("SMA 20: {}", fmt_opt(td.sma_20)));
    lines.push(format!("SMA 50: {}", fmt_opt(td.sma_50)));
    lines.push(format!("EMA 12: {}", fmt_opt(td.ema_12)));
    lines.push(format!("EMA 26: {}", fmt_opt(td.ema_26)));
    lines.push(format!("Bollinger Upper: {}", fmt_opt(td.bollinger_upper)));
    lines.push(format!("Bollinger Lower: {}", fmt_opt(td.bollinger_lower)));
    lines.push(format!("Support Level: {}", fmt_opt(td.support_level)));
    lines.push(format!(
        "Resistance Level: {}",
        fmt_opt(td.resistance_level)
    ));
    lines.push(format!("Volume Avg (VWMA 20): {}", fmt_opt(td.volume_avg)));
    lines.push(format!("\nIndicator Summary: {}", td.summary));

    lines.join("\n")
}

fn fmt_opt(v: Option<f64>) -> String {
    match v {
        Some(f) => format!("{f:.4}"),
        None => "null".to_owned(),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MacdValues, TechnicalData};

    fn parse_technical(json: &str) -> Result<TechnicalData, TradingError> {
        serde_json::from_str(json).map_err(|e| TradingError::SchemaViolation {
            message: format!("TechnicalAnalyst: failed to parse LLM output: {e}"),
        })
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

        let data = parse_technical(json).expect("should parse");
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

    #[test]
    fn format_indicator_context_uses_correct_field_names() {
        let td = TechnicalData {
            rsi: Some(55.0),
            macd: Some(MacdValues {
                macd_line: 0.1,
                signal_line: 0.05,
                histogram: 0.05,
            }),
            atr: Some(1.5),
            sma_20: Some(150.0),
            sma_50: Some(148.0),
            ema_12: Some(152.0),
            ema_26: Some(149.0),
            bollinger_upper: Some(160.0),
            bollinger_lower: Some(140.0),
            support_level: Some(145.0),
            resistance_level: Some(165.0),
            volume_avg: Some(1_000_000.0),
            summary: "Test summary.".to_owned(),
        };

        let ctx = format_indicator_context(&td);
        assert!(ctx.contains("RSI"), "should include RSI label");
        assert!(ctx.contains("MACD"), "should include MACD label");
        assert!(ctx.contains("ATR"), "should include ATR label");
        assert!(ctx.contains("Bollinger"), "should include Bollinger label");
        assert!(ctx.contains("Support"), "should include Support label");
        assert!(
            ctx.contains("Resistance"),
            "should include Resistance label"
        );
        assert!(!ctx.contains("open:"), "should NOT include raw OHLCV open");
        assert!(
            !ctx.contains("close:"),
            "should NOT include raw OHLCV close"
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

        let data = parse_technical(json).expect("should parse all-null");
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
            agent_name: "technical".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 300,
        };
        assert_eq!(usage.agent_name, "technical");
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
}
