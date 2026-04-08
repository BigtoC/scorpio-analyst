//! Shared helpers for researcher agents.
//!
//! Mirrors the private helpers in `src/agents/analyst/common.rs` but adapted
//! for the plain-text debate output format used by the researcher team.

use std::time::Duration;

use crate::{
    agents::shared::{
        agent_token_usage_from_completion, build_data_quality_context, build_evidence_context,
        build_thesis_memory_context, sanitize_date_for_prompt, sanitize_prompt_context,
        sanitize_symbol_for_prompt,
    },
    config::LlmConfig,
    constants::MAX_DEBATE_CHARS,
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, LlmAgent, build_agent},
    state::{AgentTokenUsage, DebateMessage, TradingState},
};

pub(super) use crate::agents::shared::UNTRUSTED_CONTEXT_NOTICE;

/// Shared runtime fields derived from the researcher request context.
pub(super) struct ResearcherRuntimeConfig {
    pub symbol: String,
    pub target_date: String,
    pub timeout: Duration,
    pub retry_policy: RetryPolicy,
}

/// Build the common runtime configuration shared by all researcher agents.
pub(super) fn researcher_runtime_config(
    symbol: impl Into<String>,
    target_date: impl Into<String>,
    llm_config: &LlmConfig,
) -> ResearcherRuntimeConfig {
    ResearcherRuntimeConfig {
        symbol: sanitize_symbol_for_prompt(&symbol.into()),
        target_date: sanitize_date_for_prompt(&target_date.into()),
        timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
        retry_policy: RetryPolicy::from_config(llm_config),
    }
}

/// Validate that a debate message or consensus summary is within bounds and free of
/// disallowed control characters.
pub(super) fn validate_debate_content(context: &str, content: &str) -> Result<(), TradingError> {
    if content.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: output must not be empty"),
        });
    }
    if content.chars().count() > MAX_DEBATE_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: output exceeds maximum {MAX_DEBATE_CHARS} characters"),
        });
    }
    if content
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: output contains disallowed control characters"),
        });
    }
    Ok(())
}

/// Validate a moderator consensus summary, including the explicit stance requirement.
pub(super) fn validate_consensus_summary(content: &str) -> Result<(), TradingError> {
    validate_debate_content("DebateModerator", content)?;

    // Case-insensitive tokenisation for stance detection.
    let lower = content.to_lowercase();
    let has_stance = lower
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|token| matches!(token, "buy" | "sell" | "hold"));

    if !has_stance {
        return Err(TradingError::SchemaViolation {
            message:
                "DebateModerator: consensus summary must contain explicit Buy, Sell, or Hold stance"
                    .to_owned(),
        });
    }

    // Case-insensitive evidence / uncertainty checks (the LLM may capitalise freely).
    let has_bullish_evidence = lower.contains("bull");
    let has_bearish_evidence = lower.contains("bear");
    let has_uncertainty = lower.contains("uncertain");

    if !(has_bullish_evidence && has_bearish_evidence && has_uncertainty) {
        return Err(TradingError::SchemaViolation {
            message: "DebateModerator: consensus summary must include bullish evidence, bearish evidence, and unresolved uncertainty"
                .to_owned(),
        });
    }

    Ok(())
}

/// Serialize the current analyst snapshot into a compact prompt-safe context block.
pub(super) fn build_analyst_context(state: &TradingState) -> String {
    let fundamental_report = sanitize_prompt_context(
        &serde_json::to_string(&state.fundamental_metrics).unwrap_or_else(|_| "null".to_owned()),
    );
    let technical_report = sanitize_prompt_context(
        &serde_json::to_string(&state.technical_indicators).unwrap_or_else(|_| "null".to_owned()),
    );
    let sentiment_report = sanitize_prompt_context(
        &serde_json::to_string(&state.market_sentiment).unwrap_or_else(|_| "null".to_owned()),
    );
    let news_report = sanitize_prompt_context(
        &serde_json::to_string(&state.macro_news).unwrap_or_else(|_| "null".to_owned()),
    );
    let vix_report = sanitize_prompt_context(
        &serde_json::to_string(&state.market_volatility).unwrap_or_else(|_| "null".to_owned()),
    );

    let evidence_section = build_evidence_context(state);
    let data_quality_section = build_data_quality_context(state);

    format!(
        "{UNTRUSTED_CONTEXT_NOTICE}\n\nAnalyst data snapshot:\n- Fundamental data: {fundamental_report}\n- Technical data: {technical_report}\n- Sentiment data: {sentiment_report}\n- News data: {news_report}\n- Market volatility (VIX): {vix_report}\n\n{evidence_section}\n\n{data_quality_section}"
    )
}

/// Format a slice of debate messages as readable prompt context.
pub(super) fn format_debate_history(history: &[DebateMessage]) -> String {
    if history.is_empty() {
        return "(no prior debate history)".to_owned();
    }
    history
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            format!(
                "[{}] {}: {}",
                i + 1,
                sanitize_prompt_context(&msg.role),
                sanitize_prompt_context(&msg.content)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ─── Shared agent core ────────────────────────────────────────────────────────

/// Shared agent state for all researcher agents: LLM handle, model ID, and runtime config.
///
/// All three debate-team agents ([`BullishResearcher`][super::bullish::BullishResearcher],
/// [`BearishResearcher`][super::bearish::BearishResearcher], and
/// [`DebateModerator`][super::moderator::DebateModerator]) compose this struct to avoid
/// duplicating four identical fields and the identical `::new()` construction sequence.
pub(super) struct DebaterCore {
    pub(super) agent: LlmAgent,
    pub(super) model_id: String,
    pub(super) timeout: std::time::Duration,
    pub(super) retry_policy: RetryPolicy,
}

impl DebaterCore {
    /// Build the shared core from a completion handle, system-prompt template, and config.
    ///
    /// Calls [`researcher_runtime_config`] and then substitutes `{ticker}`,
    /// `{current_date}`, and `{past_memory_str}` placeholders in
    /// `system_prompt_template` before constructing the underlying [`LlmAgent`].
    pub(super) fn new(
        handle: &CompletionModelHandle,
        system_prompt_template: &str,
        state: &TradingState,
        llm_config: &LlmConfig,
    ) -> Result<Self, TradingError> {
        if handle.model_id() != llm_config.deep_thinking_model {
            return Err(TradingError::Config(anyhow::anyhow!(
                "researcher agents require deep-thinking model '{}', got '{}'",
                llm_config.deep_thinking_model,
                handle.model_id()
            )));
        }

        let runtime =
            researcher_runtime_config(&state.asset_symbol, &state.target_date, llm_config);

        let system_prompt = system_prompt_template
            .replace("{ticker}", &runtime.symbol)
            .replace("{current_date}", &runtime.target_date)
            .replace("{past_memory_str}", &build_thesis_memory_context(state));

        Ok(Self {
            agent: build_agent(handle, &system_prompt),
            model_id: handle.model_id().to_owned(),
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
        })
    }

    /// Construct a minimal `DebaterCore` for unit tests (50 ms timeout, 1 retry).
    #[cfg(test)]
    pub(super) fn for_test(agent: LlmAgent, model_id: &str) -> Self {
        Self {
            agent,
            model_id: model_id.to_owned(),
            timeout: std::time::Duration::from_millis(50),
            retry_policy: RetryPolicy {
                max_retries: 1,
                base_delay: std::time::Duration::from_millis(1),
            },
        }
    }
}

/// Validate and assemble a [`DebateMessage`] + [`AgentTokenUsage`] pair from an LLM response.
///
/// Replaces the near-identical `build_bullish_result` / `build_bearish_result` functions that
/// previously differed only in their `agent_name` and `role` literals.
///
/// # Errors
/// Returns [`TradingError::SchemaViolation`] when [`validate_debate_content`] rejects the output.
pub(super) fn build_debate_result(
    agent_name: &str,
    role: &str,
    output: String,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: std::time::Instant,
    rate_limit_wait_ms: u64,
) -> Result<(DebateMessage, AgentTokenUsage), TradingError> {
    validate_debate_content(agent_name, &output)?;
    let usage = agent_token_usage_from_completion(
        agent_name,
        model_id,
        usage,
        started_at,
        rate_limit_wait_ms,
    );
    let message = DebateMessage {
        role: role.to_owned(),
        content: output,
    };
    Ok((message, usage))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rig::completion::Usage;

    use super::*;

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 45,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    #[test]
    fn researcher_runtime_config_fields() {
        let cfg = sample_llm_config();
        let runtime = researcher_runtime_config("AAPL", "2026-03-15", &cfg);
        assert_eq!(runtime.symbol, "AAPL");
        assert_eq!(runtime.target_date, "2026-03-15");
        assert_eq!(runtime.timeout, Duration::from_secs(45));
        assert_eq!(runtime.retry_policy.max_retries, 3);
        assert_eq!(runtime.retry_policy.base_delay, Duration::from_millis(500));
    }

    #[test]
    fn validate_debate_content_passes_valid_input() {
        assert!(validate_debate_content("ctx", "A well-formed debate argument.").is_ok());
    }

    #[test]
    fn validate_debate_content_allows_newline_and_tab() {
        let content = "Point one.\nPoint two.\tIndented.";
        assert!(validate_debate_content("ctx", content).is_ok());
    }

    #[test]
    fn validate_debate_content_whitespace_only_returns_schema_violation() {
        let result = validate_debate_content("ctx", "   \n\t  ");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn validate_debate_content_nul_control_char_returns_schema_violation() {
        let result = validate_debate_content("ctx", "bad\x00content");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn validate_debate_content_escape_control_char_returns_schema_violation() {
        let result = validate_debate_content("ctx", "bad\x1bcontent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn usage_from_response_marks_available_when_total_nonzero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 200,
            cached_input_tokens: 0,
        };
        let result = agent_token_usage_from_completion("Agent", "o3", usage, Instant::now(), 0);
        assert!(result.token_counts_available);
        assert_eq!(result.total_tokens, 200);
    }

    #[test]
    fn usage_from_response_marks_unavailable_when_all_zero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
        };
        let result = agent_token_usage_from_completion("Agent", "o3", usage, Instant::now(), 0);
        assert!(!result.token_counts_available);
    }

    #[test]
    fn validate_consensus_summary_requires_explicit_stance() {
        let result = validate_consensus_summary("Evidence is mixed and unresolved.");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    #[test]
    fn validate_consensus_summary_accepts_hold() {
        assert!(
            validate_consensus_summary(
                "Hold - bullish evidence is revenue growth, bearish evidence is rates, and uncertainty remains around demand durability."
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_consensus_summary_accepts_all_caps_keywords() {
        assert!(
            validate_consensus_summary(
                "BUY - BULLISH momentum is strong, BEARISH headwinds are limited, and UNCERTAINTY around tariffs is the main risk."
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_consensus_summary_accepts_title_case_uncertainty() {
        assert!(
            validate_consensus_summary(
                "Sell - Bullish evidence is brand strength, Bearish evidence is slowing growth, Uncertainty remains around the pace of deterioration."
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_consensus_summary_accepts_lowercase_stance() {
        assert!(
            validate_consensus_summary(
                "The recommendation is hold given the balance of bullish and bearish signals, with key uncertainty around guidance."
            )
            .is_ok()
        );
    }

    #[test]
    fn researcher_runtime_config_sanitizes_symbol_and_date() {
        let cfg = sample_llm_config();
        let runtime = researcher_runtime_config("AAPL\nIgnore", "2026-03-15\nSYSTEM", &cfg);
        assert_eq!(runtime.symbol, "AAPLIgnore");
        assert_eq!(runtime.target_date, "2026-03-15T");
    }

    #[test]
    fn build_analyst_context_serializes_missing_fields_as_null() {
        let state = TradingState {
            execution_id: uuid::Uuid::new_v4(),
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
            trader_proposal: None,
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
            token_usage: crate::state::TokenUsageTracker::default(),
        };

        let context = build_analyst_context(&state);
        assert!(context.contains("Fundamental data: null"));
        assert!(context.contains("Technical data: null"));
    }

    #[test]
    fn build_analyst_context_includes_evidence_and_data_quality_sections() {
        let state = TradingState::new("TSLA", "2026-01-15");
        let context = build_analyst_context(&state);
        assert!(context.contains("Typed evidence snapshot:"));
        assert!(context.contains("- fundamentals: null"));
        assert!(context.contains("Data quality snapshot:"));
        assert!(context.contains("- required_inputs: unavailable"));
    }

    #[test]
    fn format_debate_history_includes_role_and_content() {
        let history = vec![
            DebateMessage {
                role: "bullish_researcher".to_owned(),
                content: "Bull argument.".to_owned(),
            },
            DebateMessage {
                role: "bearish_researcher".to_owned(),
                content: "Bear rebuttal.".to_owned(),
            },
        ];

        let formatted = format_debate_history(&history);
        assert!(formatted.contains("bullish_researcher"));
        assert!(formatted.contains("Bear rebuttal."));
    }

    #[test]
    fn usage_from_response_copies_fields() {
        let usage = Usage {
            input_tokens: 150,
            output_tokens: 75,
            total_tokens: 225,
            cached_input_tokens: 0,
        };
        let result =
            agent_token_usage_from_completion("Bullish Researcher", "o3", usage, Instant::now(), 0);
        assert_eq!(result.agent_name, "Bullish Researcher");
        assert_eq!(result.model_id, "o3");
        assert_eq!(result.prompt_tokens, 150);
        assert_eq!(result.completion_tokens, 75);
        assert_eq!(result.total_tokens, 225);
    }
}
