//! Risk Moderator agent.
//!
//! Executes a single one-shot prompt after all risk discussion rounds complete
//! to synthesize the three risk perspectives into a plain-text discussion
//! summary for downstream review by the Fund Manager.

use std::time::Instant;

use crate::{
    agents::shared::agent_token_usage_from_completion,
    config::LlmConfig,
    error::TradingError,
    providers::factory::{CompletionModelHandle, prompt_with_retry_details},
    state::{AgentTokenUsage, RiskReport, TradingState},
};

#[cfg(test)]
use crate::providers::factory::LlmAgent;

use super::common::{
    DualRiskStatus, RiskAgentCore, UNTRUSTED_CONTEXT_NOTICE, build_analyst_context,
    expected_moderator_violation_sentence, format_risk_history, redact_text_for_storage,
    sanitize_date_for_prompt, sanitize_prompt_context, sanitize_symbol_for_prompt,
    validate_moderator_output,
};

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
        let policy = super::common::runtime_policy_for_agent(state, "RiskModerator")?;
        Ok(Self {
            core: RiskAgentCore::new(
                handle,
                policy,
                |bundle| bundle.risk_moderator.as_ref(),
                state,
                llm_config,
            )?,
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
    let dual_risk_status = DualRiskStatus::from_reports(
        state.conservative_risk_report.as_ref(),
        state.neutral_risk_report.as_ref(),
    );
    let violation_status = expected_moderator_violation_sentence(dual_risk_status);

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
    let dual_risk_status = DualRiskStatus::from_reports(
        state.conservative_risk_report.as_ref(),
        state.neutral_risk_report.as_ref(),
    );
    validate_moderator_output(&output, dual_risk_status)?;
    let output = redact_text_for_storage(&output);
    let token_usage = agent_token_usage_from_completion(
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

    fn sample_state() -> TradingState {
        TradingState {
            execution_id: Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            symbol: None,
            target_date: "2026-03-15".to_owned(),
            current_price: None,
            equity: None,
            crypto: None,
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
            enrichment_event_news: Default::default(),
            enrichment_consensus: Default::default(),
            data_coverage: None,
            provenance_summary: None,
            prior_thesis: None,
            current_thesis: None,
            token_usage: TokenUsageTracker::default(),
            analysis_pack_name: None,
            analysis_runtime_policy: None,
        }
    }

    fn valid_synthesis() -> &'static str {
        "Violation status: dual-risk escalation absent. The conservative reviewer flagged concerns but neutral did not. The proposal's stop-loss is too wide."
    }

    fn valid_dual_violation_synthesis() -> &'static str {
        "Violation status: dual-risk escalation present. Both conservative and neutral reviewers flagged a material violation. The proposal's stop-loss is too wide."
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
            "Violation status: dual-risk escalation absent. api_key=abcd1234 token=qwerty"
                .to_owned(),
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
    fn risk_moderator_prompt_drift_guard_forbids_deterministic_phrases() {
        let forbidden = [
            "must reject",
            "automatic rejection",
            "deterministic rejection",
            "deterministic reject",
            "deterministic safety rule",
            "required to reject",
            "mandatory rejection",
            "presumptive rejection",
        ];

        // Read the prompt from the canonical runtime source — the baseline
        // pack's `PromptBundle.risk_moderator` slot — so the drift guard
        // tracks what the runtime actually sends to the LLM, not a stale
        // legacy constant.
        let pack_prompt =
            crate::testing::baseline_pack_prompt_for_role(crate::workflow::Role::RiskModerator);
        let lower_prompt = pack_prompt.to_ascii_lowercase();
        for phrase in &forbidden {
            assert!(
                !lower_prompt.contains(phrase),
                "baseline risk_moderator prompt must not contain \"{phrase}\""
            );
        }
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
