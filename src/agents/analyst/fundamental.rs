//! Fundamental Analyst agent.
//!
//! Binds Finnhub data-fetching tools (`get_fundamentals`, `get_earnings`) to a
//! quick-thinking LLM agent so the model can call them during inference and
//! return a structured [`FundamentalData`] JSON object.
//!
//! Note: `get_insider_transactions` is intentionally **not** bound as a
//! standalone tool.  `get_fundamentals` already fetches insider data internally
//! via `tokio::try_join!` and populates `FundamentalData.insider_transactions`,
//! so binding both would double the Finnhub quota usage.

use std::time::Instant;

use rig::tool::ToolDyn;

use crate::{
    config::LlmConfig,
    data::{FinnhubClient, GetEarnings, GetFundamentals},
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent_with_tools, prompt_typed_with_retry},
    state::{AgentTokenUsage, FundamentalData},
};

use super::common::{usage_from_response, validate_summary_content};

const MAX_TOOL_TURNS: usize = 8;

/// System prompt for the Fundamental Analyst, adapted from `docs/prompts.md`.
const FUNDAMENTAL_SYSTEM_PROMPT: &str = "\
You are the Fundamental Analyst for {ticker} as of {current_date}.
Your job is to turn raw company financial data into a concise, evidence-backed `FundamentalData` JSON object.

Use only the tools bound for this run. When available, the runtime tool names are typically:
- `get_fundamentals`
- `get_earnings`

Note: `get_fundamentals` already includes insider transaction data in its response.
Do not call a separate insider-transactions tool.

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
/// Binds Finnhub tools to the LLM so it can call them during inference to
/// fetch the data it needs and produce a structured [`FundamentalData`] output.
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
            timeout: std::time::Duration::from_secs(llm_config.analyst_timeout_secs),
            retry_policy: RetryPolicy::from_config(llm_config),
        }
    }

    /// Run the analyst: bind Finnhub tools to the LLM, prompt it, parse and return output.
    ///
    /// # Errors
    ///
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns malformed JSON.
    pub async fn run(&self) -> Result<(FundamentalData, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();

        let tools: Vec<Box<dyn ToolDyn>> = vec![
            Box::new(GetFundamentals::scoped(
                self.finnhub.clone(),
                self.symbol.clone(),
            )),
            Box::new(GetEarnings::scoped(
                self.finnhub.clone(),
                self.symbol.clone(),
            )),
        ];

        let system_prompt = FUNDAMENTAL_SYSTEM_PROMPT
            .replace("{ticker}", &self.symbol)
            .replace("{current_date}", &self.target_date);

        let agent = build_agent_with_tools(&self.handle, &system_prompt, tools);

        let prompt = format!(
            "Analyse {} as of {}. Use the available tools to fetch the data you need, \
             then produce a FundamentalData JSON object.",
            self.symbol, self.target_date
        );

        let response = prompt_typed_with_retry::<FundamentalData>(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            MAX_TOOL_TURNS,
        )
        .await?;

        validate_fundamental(&response.output)?;

        let usage = usage_from_response(
            "Fundamental Analyst",
            self.handle.model_id(),
            response.usage,
            started_at,
        );

        Ok((response.output, usage))
    }
}

fn validate_fundamental(data: &FundamentalData) -> Result<(), TradingError> {
    if data.summary.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "FundamentalAnalyst: summary must not be empty".to_owned(),
        });
    }
    validate_summary_content("FundamentalAnalyst", &data.summary)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FundamentalData, InsiderTransaction, TransactionType};

    /// Parse a valid JSON string that matches `FundamentalData` schema.
    fn parse_fundamental(json: &str) -> Result<FundamentalData, TradingError> {
        serde_json::from_str(json)
            .map_err(|e| TradingError::SchemaViolation {
                message: format!("FundamentalAnalyst: failed to parse LLM output: {e}"),
            })
            .and_then(|data| validate_fundamental(&data).map(|()| data))
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
        assert_eq!(
            data.insider_transactions[1].transaction_type,
            TransactionType::S
        );
    }

    // ── Task 1.5: AgentTokenUsage recording ──────────────────────────────

    #[test]
    fn agent_token_usage_fields() {
        let usage = AgentTokenUsage {
            agent_name: "Fundamental Analyst".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 250,
        };
        assert_eq!(usage.agent_name, "Fundamental Analyst");
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
    fn whitespace_only_summary_returns_schema_violation() {
        let json = r#"{
            "revenue_growth_pct": null, "pe_ratio": null, "eps": null, "current_ratio": null,
            "debt_to_equity": null, "gross_margin": null, "net_income": null,
            "insider_transactions": [], "summary": "   "
        }"#;
        let result = parse_fundamental(json);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn extra_fields_in_json_are_rejected() {
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
        assert!(result.is_err());
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
                transaction_type: TransactionType::S,
            }],
            summary: "Strong fundamentals.".to_owned(),
        };

        let serialized = serde_json::to_string(&original).expect("serialise");
        let roundtripped: FundamentalData = serde_json::from_str(&serialized).expect("deserialise");
        assert_eq!(original, roundtripped);
    }
}
