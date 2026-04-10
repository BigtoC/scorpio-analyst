//! Conservative Risk Analyst agent.
//!
//! Focuses on capital preservation and surfaces downside risk. Returns a
//! [`RiskReport`] with `risk_level = Conservative`.

use std::time::Instant;

use rig::completion::Message;

use crate::{
    agents::shared::agent_token_usage_from_completion,
    config::LlmConfig,
    error::TradingError,
    providers::factory::{CompletionModelHandle, chat_with_retry_details},
    state::{AgentTokenUsage, RiskLevel, RiskReport, TradingState},
};

#[cfg(test)]
use crate::providers::factory::LlmAgent;

use super::common::{
    RiskAgentCore, UNTRUSTED_CONTEXT_NOTICE, extract_json_object, format_risk_history,
    initial_untrusted_history, redact_risk_report_for_storage, sanitize_prompt_context,
    validate_raw_model_output_size, validate_risk_text,
};

/// System prompt for the Conservative Risk Analyst, from `docs/prompts.md` §4.
const CONSERVATIVE_SYSTEM_PROMPT: &str = "\
You are the Conservative Risk Analyst reviewing the trader's proposal for {ticker} as of {current_date}.
Your role is to protect capital, surface downside risk, and reject weak controls.

Available inputs:
- Trader proposal: {trader_proposal}
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Risk discussion history: {risk_history}
- Aggressive's latest view: {aggressive_response}
- Neutral's latest view: {neutral_response}
- Past learnings: {past_memory_str}

Return ONLY a JSON object matching `RiskReport`:
- `risk_level`: `Conservative`
- `assessment`: concise string explaining your view
- `recommended_adjustments`: array of concrete refinements
- `flags_violation`: boolean

Instructions:
1. Focus on capital preservation, weak assumptions, downside scenarios, and insufficient controls.
2. Explicitly evaluate overbought RSI conditions, severe macroeconomic uncertainty, and high-beta / volatility exposure when the evidence is available.
3. Use concrete evidence from the proposal and analyst data.
4. Use `recommended_adjustments` for explicit risk reductions or avoidance steps.
5. Set `flags_violation` to `true` when the proposal has a material risk-control flaw or unjustified exposure.
6. Return ONLY the single JSON object required by `RiskReport`.";

/// The Conservative Risk Analyst agent.
///
/// Maintains a multi-turn chat session so each response can build on prior
/// risk discussion history each round.
pub struct ConservativeRiskAgent {
    core: RiskAgentCore,
    chat_history: Vec<Message>,
}

#[cfg(test)]
impl ConservativeRiskAgent {
    fn from_test_agent(agent: LlmAgent, model_id: &str) -> Self {
        Self {
            core: RiskAgentCore::for_test(agent, model_id),
            chat_history: Vec::new(),
        }
    }
}

impl ConservativeRiskAgent {
    /// Construct a new `ConservativeRiskAgent`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed LLM completion model handle (must be `DeepThinking` tier).
    /// - `state` – current trading state; analyst data is injected into the system prompt.
    /// - `llm_config` – LLM configuration for timeout and retry policy.
    ///
    /// # Errors
    /// Returns [`TradingError::Config`] if the handle does not use the deep-thinking model.
    pub fn new(
        handle: &CompletionModelHandle,
        state: &TradingState,
        llm_config: &LlmConfig,
    ) -> Result<Self, TradingError> {
        let core = RiskAgentCore::new(handle, CONSERVATIVE_SYSTEM_PROMPT, state, llm_config)?;
        let chat_history = initial_untrusted_history(state);
        Ok(Self { core, chat_history })
    }

    /// Execute one round of conservative risk analysis.
    ///
    /// # Parameters
    /// - `state` – current trading state (for `trader_proposal` and `risk_discussion_history`).
    /// - `aggressive_response` – the aggressive analyst's latest view, or `None`.
    /// - `neutral_response` – the neutral analyst's latest view, or `None`.
    ///
    /// # Returns
    /// A `(RiskReport, AgentTokenUsage)` pair on success.
    ///
    /// # Errors
    /// - [`TradingError::SchemaViolation`] if `state.trader_proposal` is `None`.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the output cannot be parsed or fails validation.
    pub async fn run(
        &mut self,
        state: &TradingState,
        aggressive_response: Option<&str>,
        neutral_response: Option<&str>,
    ) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
        let proposal =
            state
                .trader_proposal
                .as_ref()
                .ok_or_else(|| TradingError::SchemaViolation {
                    message: "ConservativeRiskAgent: trader_proposal is required but not set"
                        .to_owned(),
                })?;

        let started_at = Instant::now();
        let prompt =
            build_conservative_prompt(state, proposal, aggressive_response, neutral_response);

        let outcome = chat_with_retry_details(
            &self.core.agent,
            &prompt,
            &mut self.chat_history,
            self.core.timeout,
            &self.core.retry_policy,
        )
        .await?;

        build_conservative_result(
            outcome.result.output,
            &self.core.model_id,
            outcome.result.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        )
    }
}

fn build_conservative_prompt(
    state: &TradingState,
    proposal: &crate::state::TradeProposal,
    aggressive_response: Option<&str>,
    neutral_response: Option<&str>,
) -> String {
    let trader_proposal = sanitize_prompt_context(
        &serde_json::to_string(proposal).unwrap_or_else(|_| "null".to_owned()),
    );
    let risk_history = format_risk_history(&state.risk_discussion_history);
    let aggressive_text = aggressive_response
        .map(sanitize_prompt_context)
        .unwrap_or_else(|| "(none yet)".to_owned());
    let neutral_text = neutral_response
        .map(sanitize_prompt_context)
        .unwrap_or_else(|| "(none yet)".to_owned());

    format!(
        "{UNTRUSTED_CONTEXT_NOTICE}\n\nTrader proposal:\n{trader_proposal}\n\nRisk discussion history:\n{risk_history}\n\nAggressive's latest view:\n{aggressive_text}\n\nNeutral's latest view:\n{neutral_text}\n\nProvide your conservative risk analysis as a JSON `RiskReport`."
    )
}

fn build_conservative_result(
    output: String,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
    rate_limit_wait_ms: u64,
) -> Result<(RiskReport, AgentTokenUsage), TradingError> {
    validate_raw_model_output_size("ConservativeRiskAgent", &output)?;
    let cleaned = extract_json_object("ConservativeRiskAgent", &output)?;
    let report: RiskReport =
        serde_json::from_str(&cleaned).map_err(|e| TradingError::SchemaViolation {
            message: format!("ConservativeRiskAgent: failed to parse RiskReport JSON: {e}"),
        })?;

    if report.risk_level != RiskLevel::Conservative {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "ConservativeRiskAgent: expected risk_level=Conservative, got {:?}",
                report.risk_level
            ),
        });
    }

    validate_risk_text("ConservativeRiskAgent.assessment", &report.assessment)?;
    for (i, adj) in report.recommended_adjustments.iter().enumerate() {
        validate_risk_text(
            &format!("ConservativeRiskAgent.recommended_adjustments[{i}]"),
            adj,
        )?;
    }

    let report = redact_risk_report_for_storage(report);
    let token_usage = agent_token_usage_from_completion(
        "Conservative Risk Analyst",
        model_id,
        usage,
        started_at,
        rate_limit_wait_ms,
    );
    Ok((report, token_usage))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, ProviderSettings, ProvidersConfig};
    use crate::providers::factory::{MockChatOutcome, mock_llm_agent, mock_prompt_response};
    use crate::providers::{ModelTier, factory::create_completion_model};
    use crate::state::{AgentTokenUsage, TokenUsageTracker, TradeAction, TradeProposal};
    use secrecy::SecretString;
    use uuid::Uuid;

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    fn providers_config_with_openai() -> ProvidersConfig {
        ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn sample_state_with_proposal() -> TradingState {
        TradingState {
            execution_id: Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            target_date: "2026-03-15".to_owned(),
            current_price: None,
            market_volatility: None,
            fundamental_metrics: None,
            technical_indicators: None,
            market_sentiment: None,
            macro_news: None,
            debate_history: Vec::new(),
            consensus_summary: None,
            trader_proposal: Some(TradeProposal {
                action: TradeAction::Buy,
                target_price: 200.0,
                stop_loss: 180.0,
                confidence: 0.75,
                rationale: "Strong growth outlook".to_owned(),
                valuation_assessment: None,
                scenario_valuation: None,
            }),
            risk_discussion_history: Vec::new(),
            aggressive_risk_report: None,
            neutral_risk_report: None,
            conservative_risk_report: None,
            final_execution_status: None,
            evidence_fundamental: None,
            evidence_technical: None,
            evidence_sentiment: None,
            evidence_news: None,
            data_coverage: None,
            provenance_summary: None,
            prior_thesis: None,
            current_thesis: None,
            token_usage: TokenUsageTracker::default(),
            derived_valuation: None,
        }
    }

    fn valid_conservative_json() -> String {
        r#"{"risk_level":"Conservative","assessment":"Capital at risk; tighten stop-loss.","recommended_adjustments":["Reduce position by 20%"],"flags_violation":true}"#.to_owned()
    }

    fn mock_usage(total: u64) -> rig::completion::Usage {
        rig::completion::Usage {
            input_tokens: total / 2,
            output_tokens: total / 2,
            total_tokens: total,
            cached_input_tokens: 0,
        }
    }

    // ── Task 2.4: Correct RiskReport construction ────────────────────────

    #[tokio::test]
    async fn run_returns_conservative_risk_report() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![],
            vec![MockChatOutcome::Ok(mock_prompt_response(
                &valid_conservative_json(),
                mock_usage(20),
            ))],
        );
        let mut analyst = ConservativeRiskAgent::from_test_agent(agent, "o3");
        let state = sample_state_with_proposal();

        let (report, _) = analyst.run(&state, None, None).await.unwrap();
        assert_eq!(report.risk_level, RiskLevel::Conservative);
        assert!(!report.assessment.is_empty());
        assert!(report.flags_violation);
    }

    // ── Task 2.5: Wrong risk_level rejected ──────────────────────────────

    #[tokio::test]
    async fn run_rejects_wrong_risk_level() {
        let wrong_json = r#"{"risk_level":"Aggressive","assessment":"Fine.","recommended_adjustments":[],"flags_violation":false}"#;
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![],
            vec![MockChatOutcome::Ok(mock_prompt_response(
                wrong_json,
                mock_usage(20),
            ))],
        );
        let mut analyst = ConservativeRiskAgent::from_test_agent(agent, "o3");
        let state = sample_state_with_proposal();

        let result = analyst.run(&state, None, None).await;
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    // ── Task 2.6: AgentTokenUsage agent name ─────────────────────────────

    #[tokio::test]
    async fn run_records_correct_agent_name() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![],
            vec![MockChatOutcome::Ok(mock_prompt_response(
                &valid_conservative_json(),
                mock_usage(30),
            ))],
        );
        let mut analyst = ConservativeRiskAgent::from_test_agent(agent, "o3");
        let state = sample_state_with_proposal();

        let (_, usage) = analyst.run(&state, None, None).await.unwrap();
        assert_eq!(usage.agent_name, "Conservative Risk Analyst");
        assert_eq!(usage.model_id, "o3");
    }

    // ── Task 2.7: assessment / recommended_adjustments validation ─────────

    #[test]
    fn build_conservative_result_rejects_empty_assessment() {
        let json = r#"{"risk_level":"Conservative","assessment":"","recommended_adjustments":["ok"],"flags_violation":false}"#;
        let result = build_conservative_result(
            json.to_owned(),
            "o3",
            rig::completion::Usage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                cached_input_tokens: 0,
            },
            Instant::now(),
            0,
        );
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn build_conservative_result_rejects_control_char_in_adjustment() {
        let json = r#"{"risk_level":"Conservative","assessment":"Caution.","recommended_adjustments":["bad\u0000entry"],"flags_violation":false}"#;
        let result = build_conservative_result(
            json.to_owned(),
            "o3",
            rig::completion::Usage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                cached_input_tokens: 0,
            },
            Instant::now(),
            0,
        );
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn build_conservative_prompt_includes_untrusted_notice() {
        let state = sample_state_with_proposal();
        let proposal = state.trader_proposal.as_ref().unwrap().clone();
        let prompt = build_conservative_prompt(&state, &proposal, None, None);
        assert!(prompt.contains(UNTRUSTED_CONTEXT_NOTICE));
    }

    #[test]
    fn build_conservative_prompt_includes_aggressive_and_neutral_views() {
        let state = sample_state_with_proposal();
        let proposal = state.trader_proposal.as_ref().unwrap().clone();
        let prompt = build_conservative_prompt(
            &state,
            &proposal,
            Some("Proceed boldly"),
            Some("Balanced view"),
        );
        assert!(prompt.contains("Proceed boldly"));
        assert!(prompt.contains("Balanced view"));
    }

    #[test]
    fn conservative_system_prompt_mentions_rsi_macro_and_beta_risks() {
        assert!(CONSERVATIVE_SYSTEM_PROMPT.contains("RSI"));
        assert!(CONSERVATIVE_SYSTEM_PROMPT.contains("macroeconomic"));
        assert!(CONSERVATIVE_SYSTEM_PROMPT.contains("beta"));
    }

    #[test]
    fn build_conservative_result_rejects_malformed_json() {
        let result = build_conservative_result(
            "not json".to_owned(),
            "o3",
            mock_usage(2),
            Instant::now(),
            0,
        );
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn build_conservative_result_redacts_secret_from_stored_output() {
        let json = r#"{"risk_level":"Conservative","assessment":"api_key=abcd1234","recommended_adjustments":["token=qwerty"],"flags_violation":true}"#;
        let (report, _) =
            build_conservative_result(json.to_owned(), "o3", mock_usage(2), Instant::now(), 0)
                .unwrap();
        assert_eq!(report.assessment, "api_key=[REDACTED]");
        assert_eq!(report.recommended_adjustments, vec!["token=[REDACTED]"]);
    }

    #[test]
    fn constructor_rejects_quick_thinking_handle() {
        let cfg = sample_llm_config();
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openai(),
            &crate::rate_limit::ProviderRateLimiters::default(),
        )
        .unwrap();
        let state = sample_state_with_proposal();
        let result = ConservativeRiskAgent::new(&handle, &state, &cfg);
        assert!(matches!(result, Err(TradingError::Config(_))));
    }

    #[test]
    fn agent_token_usage_has_correct_agent_name() {
        let usage = AgentTokenUsage {
            agent_name: "Conservative Risk Analyst".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 5,
            rate_limit_wait_ms: 0,
        };
        assert_eq!(usage.agent_name, "Conservative Risk Analyst");
    }
}
