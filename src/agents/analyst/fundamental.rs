//! Fundamental Analyst agent.
//!
//! Fetches raw financial data from Finnhub (fundamentals, earnings, insider
//! transactions), serialises it as context, and passes it to a quick-thinking
//! LLM agent that returns a structured [`FundamentalData`] JSON object.

use std::time::Instant;

use crate::{
    config::LlmConfig,
    data::FinnhubClient,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent, prompt_with_retry},
    state::{AgentTokenUsage, FundamentalData},
};

/// System prompt for the Fundamental Analyst, adapted from `docs/prompts.md`.
const FUNDAMENTAL_SYSTEM_PROMPT: &str = "\
You are the Fundamental Analyst for {ticker} as of {current_date}.
Your job is to turn raw company financial data into a concise, evidence-backed `FundamentalData` JSON object.

Use only the tools bound for this run. When available, the runtime tool names are typically:
- `get_fundamentals`
- `get_earnings`
- `get_insider_transactions`

Populate only these schema fields:
- `revenue_growth_pct`
- `pe_ratio`
- `eps`
- `current_ratio`
- `debt_to_equity`
- `gross_margin`
- `net_income`
- `insider_transactions`
- `summary`

Instructions:
1. Gather enough data to evaluate growth, valuation, profitability, liquidity, leverage, and insider activity.
2. Base every populated numeric field on tool output. If a value is unavailable, return `null` for that field.
3. Populate `insider_transactions` only with actual records from tool output. If none are available, return `[]`.
4. Keep `summary` short and useful for downstream agents. It should explain what matters, not restate every metric.
5. Do not invent management guidance, free-cash-flow commentary, or any metric not present in the runtime schema.
6. Return ONLY the single JSON object required by `FundamentalData`.

Do not include any trade recommendation, target price, or final transaction proposal.";

/// The Fundamental Analyst agent.
///
/// Pre-fetches financial data from Finnhub, then invokes an LLM to interpret
/// that data and produce a structured [`FundamentalData`] output.
pub struct FundamentalAnalyst {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    symbol: String,
    target_date: String,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl FundamentalAnalyst {
    /// Construct a new `FundamentalAnalyst`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (should be `QuickThinking` tier).
    /// - `finnhub` – Finnhub client for data fetching.
    /// - `symbol` – asset ticker symbol (e.g. `"AAPL"`).
    /// - `target_date` – analysis date string (e.g. `"2026-03-14"`).
    /// - `llm_config` – LLM configuration, used for timeout.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        symbol: impl Into<String>,
        target_date: impl Into<String>,
        llm_config: &LlmConfig,
    ) -> Self {
        Self {
            handle,
            finnhub,
            symbol: symbol.into(),
            target_date: target_date.into(),
            timeout: std::time::Duration::from_secs(llm_config.agent_timeout_secs),
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Run the analyst: fetch data, prompt the LLM, parse and return output.
    ///
    /// # Errors
    ///
    /// - [`TradingError::AnalystError`] when Finnhub data fetching fails.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns malformed JSON.
    pub async fn run(&self) -> Result<(FundamentalData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        // ── 1. Fetch data from Finnhub ────────────────────────────────────
        let (fundamentals, earnings, insider) = tokio::try_join!(
            self.finnhub.get_fundamentals(&self.symbol),
            self.finnhub.get_earnings(&self.symbol),
            self.finnhub.get_insider_transactions(&self.symbol),
        )
        .map_err(|e| TradingError::AnalystError {
            agent: "fundamental".to_owned(),
            message: e.to_string(),
        })?;

        // ── 2. Serialize context ──────────────────────────────────────────
        let context = format!(
            "## Fundamentals\n{}\n\n## Earnings (latest EPS)\n{}\n\n## Insider Transactions\n{}",
            serde_json::to_string_pretty(&fundamentals).unwrap_or_default(),
            serde_json::to_string_pretty(&earnings).unwrap_or_default(),
            serde_json::to_string_pretty(&insider).unwrap_or_default(),
        );

        // ── 3. Build agent and invoke LLM ─────────────────────────────────
        let system_prompt = FUNDAMENTAL_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent(&self.handle, &system_prompt);

        let prompt = format!(
            "Using the financial data below, produce a `FundamentalData` JSON object for {} as of {}.\n\n{}",
            self.symbol, self.target_date, context
        );

        let raw = prompt_with_retry(&agent, &prompt, self.timeout, &self.retry_policy).await?;

        // ── 4. Parse structured output ────────────────────────────────────
        let data: FundamentalData =
            serde_json::from_str(raw.trim()).map_err(|e| TradingError::SchemaViolation {
                message: format!("FundamentalAnalyst: failed to parse LLM output: {e}"),
            })?;

        // ── 5. Record token usage (counts unavailable from provider) ───────
        let latency_ms = started_at.elapsed().as_millis() as u64;
        let usage = AgentTokenUsage {
            agent_name: "fundamental".to_owned(),
            model_id: self.handle.model_id().to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms,
        };

        Ok((data, usage))
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FundamentalData, InsiderTransaction};

    /// Parse a valid JSON string that matches `FundamentalData` schema.
    fn parse_fundamental(json: &str) -> Result<FundamentalData, TradingError> {
        serde_json::from_str(json).map_err(|e| TradingError::SchemaViolation {
            message: format!("FundamentalAnalyst: failed to parse LLM output: {e}"),
        })
    }

    // ── Task 1.4: Correct FundamentalData extraction ─────────────────────

    #[test]
    fn parses_valid_fundamental_json() {
        let json = r#"{
            "revenue_growth_pct": 0.12,
            "pe_ratio": 28.5,
            "eps": 6.1,
            "current_ratio": 1.3,
            "debt_to_equity": 0.8,
            "gross_margin": 0.43,
            "net_income": 95000000000.0,
            "insider_transactions": [
                {
                    "name": "Tim Cook",
                    "share_change": -5000.0,
                    "transaction_date": "2026-01-10",
                    "transaction_type": "S"
                }
            ],
            "summary": "AAPL shows strong margins and moderate leverage."
        }"#;

        let data = parse_fundamental(json).expect("should parse valid JSON");
        assert_eq!(data.pe_ratio, Some(28.5));
        assert_eq!(data.eps, Some(6.1));
        assert_eq!(data.gross_margin, Some(0.43));
        assert_eq!(data.insider_transactions.len(), 1);
        assert_eq!(data.insider_transactions[0].name, "Tim Cook");
        assert!(!data.summary.is_empty());
    }

    #[test]
    fn parses_fundamental_with_null_fields() {
        let json = r#"{
            "revenue_growth_pct": null,
            "pe_ratio": null,
            "eps": null,
            "current_ratio": null,
            "debt_to_equity": null,
            "gross_margin": null,
            "net_income": null,
            "insider_transactions": [],
            "summary": "No data available."
        }"#;

        let data = parse_fundamental(json).expect("should parse nulls");
        assert!(data.pe_ratio.is_none());
        assert!(data.eps.is_none());
        assert!(data.insider_transactions.is_empty());
    }

    #[test]
    fn parses_fundamental_with_multiple_insider_transactions() {
        let json = r#"{
            "revenue_growth_pct": 0.05,
            "pe_ratio": 20.0,
            "eps": 3.0,
            "current_ratio": 1.5,
            "debt_to_equity": 0.5,
            "gross_margin": 0.35,
            "net_income": 10000000000.0,
            "insider_transactions": [
                {"name": "Alice", "share_change": 1000.0, "transaction_date": "2026-01-01", "transaction_type": "P"},
                {"name": "Bob", "share_change": -500.0, "transaction_date": "2026-01-05", "transaction_type": "S"}
            ],
            "summary": "Solid fundamentals."
        }"#;

        let data = parse_fundamental(json).expect("should parse");
        assert_eq!(data.insider_transactions.len(), 2);
        assert_eq!(data.insider_transactions[0].name, "Alice");
        assert_eq!(data.insider_transactions[1].transaction_type, "S");
    }

    // ── Task 1.5: AgentTokenUsage recording ──────────────────────────────

    #[test]
    fn agent_token_usage_fields() {
        let usage = AgentTokenUsage {
            agent_name: "fundamental".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 250,
        };
        assert_eq!(usage.agent_name, "fundamental");
        assert_eq!(usage.model_id, "gpt-4o-mini");
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage.latency_ms > 0);
    }

    // ── Task 1.6: SchemaViolation on malformed JSON ───────────────────────

    #[test]
    fn malformed_json_returns_schema_violation() {
        let result = parse_fundamental("not valid json at all");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn json_missing_required_field_returns_schema_violation() {
        // `summary` is a required non-nullable String — omitting it should fail
        let result = parse_fundamental(r#"{"pe_ratio": 20.0}"#);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn extra_fields_in_json_are_ignored() {
        // serde ignores unknown fields by default
        let json = r#"{
            "revenue_growth_pct": null,
            "pe_ratio": null,
            "eps": null,
            "current_ratio": null,
            "debt_to_equity": null,
            "gross_margin": null,
            "net_income": null,
            "insider_transactions": [],
            "summary": "ok",
            "unexpected_field": "ignored"
        }"#;
        let result = parse_fundamental(json);
        assert!(result.is_ok());
    }

    // ── Struct round-trip ─────────────────────────────────────────────────

    #[test]
    fn fundamental_data_round_trips_through_json() {
        let original = FundamentalData {
            revenue_growth_pct: Some(0.12),
            pe_ratio: Some(28.5),
            eps: Some(6.1),
            current_ratio: Some(1.3),
            debt_to_equity: None,
            gross_margin: Some(0.43),
            net_income: Some(9.5e10),
            insider_transactions: vec![InsiderTransaction {
                name: "Jane".to_owned(),
                share_change: -1000.0,
                transaction_date: "2026-01-01".to_owned(),
                transaction_type: "S".to_owned(),
            }],
            summary: "Strong fundamentals.".to_owned(),
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: FundamentalData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }
}
