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
    agents::shared::agent_token_usage_from_completion,
    config::LlmConfig,
    constants::FUNDAMENTAL_ANALYST_MAX_TURNS,
    data::{FinnhubClient, GetEarnings, GetFundamentals},
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, build_agent_with_tools},
    state::{AgentTokenUsage, FundamentalData},
};

use super::common::{analyst_runtime_config, run_analyst_inference, validate_summary_content};

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
6. Return exactly one JSON object required by `FundamentalData`. No prose, no markdown fences — output exactly one JSON object, no prose, no markdown fences.

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
        let runtime = analyst_runtime_config(symbol, target_date, llm_config);

        Self {
            handle,
            finnhub,
            symbol: runtime.symbol,
            target_date: runtime.target_date,
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
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

        let outcome = run_analyst_inference(
            &agent,
            &prompt,
            self.timeout,
            &self.retry_policy,
            FUNDAMENTAL_ANALYST_MAX_TURNS,
            parse_fundamental,
            validate_fundamental,
        )
        .await?;

        let usage = agent_token_usage_from_completion(
            "Fundamental Analyst",
            self.handle.model_id(),
            outcome.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        Ok((outcome.output, usage))
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

/// Deserialize a JSON string into [`FundamentalData`], mapping errors to
/// [`TradingError::SchemaViolation`].
///
/// Exposed for use as the `parse` hook in `run_analyst_inference`.
pub(crate) fn parse_fundamental(json_str: &str) -> Result<FundamentalData, TradingError> {
    serde_json::from_str(json_str).map_err(|e| TradingError::SchemaViolation {
        message: format!("FundamentalAnalyst: failed to parse LLM output: {e}"),
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FundamentalData, InsiderTransaction, TransactionType};

    /// Parse and validate a JSON string — combines `parse_fundamental` + `validate_fundamental`
    /// for test convenience.  Tests that need only structural parsing can call `parse_fundamental`
    /// directly; tests that also exercise the semantic validation layer call this helper.
    fn parse_and_validate(json: &str) -> Result<FundamentalData, TradingError> {
        parse_fundamental(json).and_then(|data| validate_fundamental(&data).map(|()| data))
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
            rate_limit_wait_ms: 0,
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
        let result = parse_and_validate(json);
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

    // TC-19: TransactionType::P deserialises from "P"
    #[test]
    fn transaction_type_p_deserializes() {
        let json = r#"{
            "revenue_growth_pct": null, "pe_ratio": null, "eps": null, "current_ratio": null,
            "debt_to_equity": null, "gross_margin": null, "net_income": null,
            "insider_transactions": [
                {"name": "Alice", "share_change": 500.0, "transaction_date": "2026-02-01", "transaction_type": "P"}
            ],
            "summary": "Purchase recorded."
        }"#;
        let data = parse_fundamental(json).expect("should parse");
        assert_eq!(
            data.insider_transactions[0].transaction_type,
            TransactionType::P
        );
    }

    // TC-3: TransactionType::Other deserialises from an unknown code like "G" (gift)
    #[test]
    fn transaction_type_other_deserializes_unknown_code() {
        let json = r#"{
            "revenue_growth_pct": null, "pe_ratio": null, "eps": null, "current_ratio": null,
            "debt_to_equity": null, "gross_margin": null, "net_income": null,
            "insider_transactions": [
                {"name": "Bob", "share_change": 100.0, "transaction_date": "2026-03-01", "transaction_type": "G"}
            ],
            "summary": "Gift transaction captured as Other."
        }"#;
        let data = parse_fundamental(json).expect("should parse");
        assert_eq!(
            data.insider_transactions[0].transaction_type,
            TransactionType::Other,
            "unknown transaction code 'G' should deserialise to TransactionType::Other"
        );
    }

    // TC-15: InsiderTransaction rejects extra fields (deny_unknown_fields)
    #[test]
    fn insider_transaction_extra_fields_rejected() {
        let json = r#"{
            "revenue_growth_pct": null, "pe_ratio": null, "eps": null, "current_ratio": null,
            "debt_to_equity": null, "gross_margin": null, "net_income": null,
            "insider_transactions": [
                {
                    "name": "Eve", "share_change": 200.0, "transaction_date": "2026-01-15",
                    "transaction_type": "S", "unexpected_field": "rejected"
                }
            ],
            "summary": "Should fail."
        }"#;
        let result = parse_fundamental(json);
        assert!(
            result.is_err(),
            "extra field inside InsiderTransaction should be rejected"
        );
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── Task 3: Migrate to shared inference helper ────────────────────────

    #[test]
    fn fundamental_prompt_requires_exactly_one_json_object_response() {
        assert!(
            FUNDAMENTAL_SYSTEM_PROMPT.contains("exactly one JSON object"),
            "FUNDAMENTAL_SYSTEM_PROMPT must contain 'exactly one JSON object'"
        );
        assert!(
            FUNDAMENTAL_SYSTEM_PROMPT.contains("no prose"),
            "FUNDAMENTAL_SYSTEM_PROMPT must contain 'no prose'"
        );
        assert!(
            FUNDAMENTAL_SYSTEM_PROMPT.contains("no markdown fences"),
            "FUNDAMENTAL_SYSTEM_PROMPT must contain 'no markdown fences'"
        );
    }

    #[test]
    fn parse_fundamental_rejects_unknown_fields() {
        let result = parse_fundamental(r#"{"unknown_field": 1}"#);
        assert!(
            matches!(result, Err(TradingError::SchemaViolation { .. })),
            "parse_fundamental should return SchemaViolation for unknown fields"
        );
    }

    #[tokio::test]
    async fn fundamental_run_uses_shared_inference_helper_for_openrouter() {
        use super::super::common::run_analyst_inference;
        use crate::providers::ProviderId;
        use crate::providers::factory::agent_test_support;
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let valid_json = r#"{
            "revenue_growth_pct": 0.10,
            "pe_ratio": 25.0,
            "eps": 5.0,
            "current_ratio": 1.2,
            "debt_to_equity": 0.6,
            "gross_margin": 0.40,
            "net_income": 80000000000.0,
            "insider_transactions": [],
            "summary": "Solid growth with healthy margins."
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

        let outcome = run_analyst_inference(
            &agent,
            "analyse AAPL",
            std::time::Duration::from_millis(100),
            &crate::error::RetryPolicy {
                max_retries: 0,
                base_delay: std::time::Duration::from_millis(1),
            },
            1,
            parse_fundamental,
            validate_fundamental,
        )
        .await
        .expect("inference should succeed");

        let _ = outcome.output;

        assert_eq!(agent_test_support::typed_attempts(&agent), 0);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }
}
