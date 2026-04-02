//! Risk Moderator agent.
//!
//! Executes a single one-shot prompt after all risk discussion rounds complete
//! to synthesize the three risk perspectives into a plain-text discussion
//! summary for downstream review by the Fund Manager.

use std::time::Instant;

use crate::{
    config::LlmConfig,
    error::TradingError,
    providers::factory::{CompletionModelHandle, prompt_with_retry_details},
    state::{AgentTokenUsage, RiskReport, TradingState},
};

#[cfg(test)]
use crate::providers::factory::LlmAgent;

use super::common::{
    RiskAgentCore, UNTRUSTED_CONTEXT_NOTICE, build_analyst_context,
    expected_moderator_violation_sentence, format_risk_history, redact_text_for_storage,
    sanitize_date_for_prompt, sanitize_prompt_context, sanitize_symbol_for_prompt,
    usage_from_response, validate_moderator_output,
};

/// System prompt for the Risk Moderator, from `docs/prompts.md` §4.
const RISK_MODERATOR_SYSTEM_PROMPT: &str = "\
You are the Risk Moderator for {ticker} as of {current_date}.
Your role is to synthesize the three risk perspectives into a concise plain-text discussion summary for downstream review.

Available inputs:
- Trader proposal: {trader_proposal}
- Aggressive risk report: {aggressive_case}
- Neutral risk report: {neutral_case}
- Conservative risk report: {conservative_case}
- Risk discussion history: {risk_history}
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Past learnings: {past_memory_str}

Instructions:
1. Identify the main agreement points and the true blockers.
2. Call out whether the trader's proposal is adequately defended on target, stop, and confidence.
3. Explicitly note whether Conservative and Neutral both flag a material violation, because the Fund Manager uses that as
   a deterministic rejection rule.
4. Keep the output concise and suitable for storage as a plain-text risk discussion note.
5. Do not output JSON and do not make the final execution decision.

Return plain text only.";

/// The Risk Moderator agent.
///
/// Uses a one-shot prompt (not multi-turn chat) because it evaluates the
/// completed risk discussion at once after all rounds have finished.
pub struct RiskModerator {
    core: RiskAgentCore,
}

#[cfg(test)]
impl RiskModerator {
    pub(super) fn from_test_agent(agent: LlmAgent, model_id: &str) -> Self {
        Self {
            core: RiskAgentCore::for_test(agent, model_id),
        }
    }
}

impl RiskModerator {
    /// Construct a new `RiskModerator`.
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
        Ok(Self {
            core: RiskAgentCore::new(handle, RISK_MODERATOR_SYSTEM_PROMPT, state, llm_config)?,
        })
    }

    /// Run the risk moderator: produce a plain-text synthesis of the risk discussion.
    ///
    /// # Parameters
    /// - `state` – current trading state with all risk reports and discussion history.
    ///
    /// # Returns
    /// A `(String, AgentTokenUsage)` pair where the string is the synthesis.
    ///
    /// # Errors
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the output is empty, oversized, or contains
    ///   disallowed control characters.
    pub async fn run(
        &self,
        state: &TradingState,
    ) -> Result<(String, AgentTokenUsage), TradingError> {
        let started_at = Instant::now();
        let prompt = build_moderator_prompt(state);

        let outcome = prompt_with_retry_details(
            &self.core.agent,
            &prompt,
            self.core.timeout,
            &self.core.retry_policy,
        )
        .await?;

        build_moderator_result(
            outcome.result.output,
            state,
            &self.core.model_id,
            outcome.result.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        )
    }
}

fn format_report(report: Option<&RiskReport>) -> String {
    report
        .map(|r| {
            sanitize_prompt_context(&serde_json::to_string(r).unwrap_or_else(|_| "null".to_owned()))
        })
        .unwrap_or_else(|| "(not yet produced)".to_owned())
}

fn build_moderator_prompt(state: &TradingState) -> String {
    let trader_proposal = state
        .trader_proposal
        .as_ref()
        .map(|p| {
            sanitize_prompt_context(&serde_json::to_string(p).unwrap_or_else(|_| "null".to_owned()))
        })
        .unwrap_or_else(|| "(no trade proposal)".to_owned());

    let aggressive_case = format_report(state.aggressive_risk_report.as_ref());
    let neutral_case = format_report(state.neutral_risk_report.as_ref());
    let conservative_case = format_report(state.conservative_risk_report.as_ref());
    let risk_history = format_risk_history(&state.risk_discussion_history);
    let analyst_context = build_analyst_context(state);
    let symbol = sanitize_symbol_for_prompt(&state.asset_symbol);
    let target_date = sanitize_date_for_prompt(&state.target_date);
    let expect_both_violation = state
        .conservative_risk_report
        .as_ref()
        .is_some_and(|r| r.flags_violation)
        && state
            .neutral_risk_report
            .as_ref()
            .is_some_and(|r| r.flags_violation);
    let violation_status = expected_moderator_violation_sentence(expect_both_violation);

    format!(
        "Synthesise the risk discussion for {} as of {}.\n\n{}\n\n{}\n\nRequired sentence to include verbatim:\n{}\n\nTrader proposal:\n{}\n\nAggressive risk report:\n{}\n\nNeutral risk report:\n{}\n\nConservative risk report:\n{}\n\nRisk discussion history:\n{}",
        symbol,
        target_date,
        UNTRUSTED_CONTEXT_NOTICE,
        analyst_context,
        violation_status,
        trader_proposal,
        aggressive_case,
        neutral_case,
        conservative_case,
        risk_history,
    )
}

fn build_moderator_result(
    output: String,
    state: &TradingState,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
    rate_limit_wait_ms: u64,
) -> Result<(String, AgentTokenUsage), TradingError> {
    let expect_both_violation = state
        .conservative_risk_report
        .as_ref()
        .is_some_and(|r| r.flags_violation)
        && state
            .neutral_risk_report
            .as_ref()
            .is_some_and(|r| r.flags_violation);
    validate_moderator_output(&output, expect_both_violation)?;
    let output = redact_text_for_storage(&output);
    let token_usage = usage_from_response(
        "Risk Moderator",
        model_id,
        usage,
        started_at,
        rate_limit_wait_ms,
    );
    Ok((output, token_usage))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, ProviderSettings, ProvidersConfig};
    use crate::providers::factory::{mock_llm_agent, mock_prompt_response};
    use crate::providers::{ModelTier, factory::create_completion_model};
    use crate::state::{
        AgentTokenUsage, RiskLevel, RiskReport, TokenUsageTracker, TradeAction, TradeProposal,
    };
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

    fn sample_state() -> TradingState {
        TradingState {
            execution_id: Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            target_date: "2026-03-15".to_owned(),
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
            }),
            risk_discussion_history: Vec::new(),
            aggressive_risk_report: Some(RiskReport {
                risk_level: RiskLevel::Aggressive,
                assessment: "Upside dominates.".to_owned(),
                recommended_adjustments: vec![],
                flags_violation: false,
            }),
            neutral_risk_report: Some(RiskReport {
                risk_level: RiskLevel::Neutral,
                assessment: "Balanced view.".to_owned(),
                recommended_adjustments: vec![],
                flags_violation: false,
            }),
            conservative_risk_report: Some(RiskReport {
                risk_level: RiskLevel::Conservative,
                assessment: "Capital at risk.".to_owned(),
                recommended_adjustments: vec!["Reduce size".to_owned()],
                flags_violation: true,
            }),
            final_execution_status: None,
            token_usage: TokenUsageTracker::default(),
        }
    }

    fn valid_synthesis() -> &'static str {
        "Violation status: Conservative and Neutral do not both flag a material violation. The proposal's stop-loss is too wide. Aggressive disagrees but evidence for upside is thin."
    }

    fn valid_dual_violation_synthesis() -> &'static str {
        "Violation status: Conservative and Neutral both flag a material violation. The proposal's stop-loss is too wide. Aggressive disagrees but evidence for upside is thin."
    }

    fn mock_usage(total: u64) -> rig::completion::Usage {
        rig::completion::Usage {
            input_tokens: total / 2,
            output_tokens: total / 2,
            total_tokens: total,
            cached_input_tokens: 0,
        }
    }

    // ── Task 4.4: Produces non-empty discussion synthesis ────────────────

    #[tokio::test]
    async fn run_produces_nonempty_synthesis() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response(valid_synthesis(), mock_usage(40)))],
            vec![],
        );
        let moderator = RiskModerator::from_test_agent(agent, "o3");
        let (synthesis, _) = moderator.run(&sample_state()).await.unwrap();
        assert!(!synthesis.is_empty());
    }

    // ── Task 4.5: Output references Conservative+Neutral violation check ─

    #[tokio::test]
    async fn run_synthesis_mentions_conservative_and_neutral_violation() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response(
                valid_dual_violation_synthesis(),
                mock_usage(40),
            ))],
            vec![],
        );
        let moderator = RiskModerator::from_test_agent(agent, "o3");
        let mut state = sample_state();
        state.neutral_risk_report.as_mut().unwrap().flags_violation = true;
        let (synthesis, _) = moderator.run(&state).await.unwrap();
        let lower = synthesis.to_lowercase();
        assert!(
            lower.contains("conservative") && lower.contains("neutral"),
            "Expected synthesis to mention both Conservative and Neutral, got: {synthesis}"
        );
    }

    // ── Task 4.6: AgentTokenUsage agent name ────────────────────────────

    #[tokio::test]
    async fn run_records_correct_agent_name() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response(valid_synthesis(), mock_usage(40)))],
            vec![],
        );
        let moderator = RiskModerator::from_test_agent(agent, "o3");
        let (_, usage) = moderator.run(&sample_state()).await.unwrap();
        assert_eq!(usage.agent_name, "Risk Moderator");
        assert_eq!(usage.model_id, "o3");
    }

    // ── Task 4.7: Oversized / control-char output rejected ───────────────

    #[tokio::test]
    async fn run_rejects_missing_required_violation_sentence() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response(
                "Summary without the required sentence.",
                mock_usage(10),
            ))],
            vec![],
        );
        let moderator = RiskModerator::from_test_agent(agent, "o3");
        let result = moderator.run(&sample_state()).await;
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[tokio::test]
    async fn run_rejects_control_char_output() {
        let (agent, _ctrl) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response("bad\x00output", mock_usage(10)))],
            vec![],
        );
        let moderator = RiskModerator::from_test_agent(agent, "o3");
        let result = moderator.run(&sample_state()).await;
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn build_moderator_prompt_includes_untrusted_notice() {
        let prompt = build_moderator_prompt(&sample_state());
        assert!(prompt.contains(UNTRUSTED_CONTEXT_NOTICE));
    }

    #[test]
    fn build_moderator_prompt_includes_all_risk_reports() {
        let prompt = build_moderator_prompt(&sample_state());
        assert!(prompt.contains("Upside dominates"));
        assert!(prompt.contains("Balanced view"));
        assert!(prompt.contains("Capital at risk"));
    }

    #[test]
    fn build_moderator_result_constructs_usage() {
        let started_at = Instant::now();
        let (synthesis, usage) = build_moderator_result(
            valid_synthesis().to_owned(),
            &sample_state(),
            "o3",
            rig::completion::Usage {
                input_tokens: 20,
                output_tokens: 10,
                total_tokens: 30,
                cached_input_tokens: 0,
            },
            started_at,
            0,
        )
        .unwrap();
        assert!(!synthesis.is_empty());
        assert_eq!(usage.agent_name, "Risk Moderator");
        assert!(usage.token_counts_available);
    }

    #[test]
    fn build_moderator_prompt_sanitizes_symbol_and_date() {
        let mut state = sample_state();
        state.asset_symbol = "AAPL\nSYSTEM".to_owned();
        state.target_date = "2026-03-15\nOVERRIDE".to_owned();
        let prompt = build_moderator_prompt(&state);
        assert!(prompt.contains("AAPLSYSTEM"));
        assert!(!prompt.contains("\nOVERRIDE"));
    }

    #[test]
    fn build_moderator_result_redacts_secret_from_stored_output() {
        let (synthesis, _) = build_moderator_result(
            "Violation status: Conservative and Neutral do not both flag a material violation. api_key=abcd1234 token=qwerty".to_owned(),
            &sample_state(),
            "o3",
            mock_usage(10),
            Instant::now(),
            0,
        )
        .unwrap();
        assert!(!synthesis.contains("abcd1234"));
        assert!(!synthesis.contains("qwerty"));
    }

    #[test]
    fn agent_token_usage_has_correct_agent_name() {
        let usage = AgentTokenUsage {
            agent_name: "Risk Moderator".to_owned(),
            model_id: "o3".to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms: 5,
            rate_limit_wait_ms: 0,
        };
        assert_eq!(usage.agent_name, "Risk Moderator");
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
        let result = RiskModerator::new(&handle, &sample_state(), &cfg);
        assert!(matches!(result, Err(TradingError::Config(_))));
    }
}
