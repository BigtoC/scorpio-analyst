//! Shared helpers for risk management agents.
//!
//! Private to the `risk` module; not re-exported publicly.

use std::time::Duration;

use rig::completion::Message;
use rig::{OneOrMany, message::UserContent};

#[cfg(test)]
use crate::agents::shared::agent_token_usage_from_completion;
use crate::{
    agents::shared::redact_secret_like_values,
    config::LlmConfig,
    constants::{MAX_RAW_MODEL_OUTPUT_CHARS, MAX_RISK_CHARS, MAX_RISK_HISTORY_CHARS},
    error::{RetryPolicy, TradingError},
    providers::factory::{CompletionModelHandle, LlmAgent, build_agent},
    state::{DebateMessage, RiskReport, TradingState},
};

pub(super) use crate::agents::shared::{
    UNTRUSTED_CONTEXT_NOTICE, extract_json_object, sanitize_date_for_prompt,
    sanitize_prompt_context, sanitize_symbol_for_prompt,
};

/// Maximum number of recent discussion messages to reinject into prompts.
const MAX_RISK_HISTORY_MESSAGES: usize = 8;

// ─── Runtime config ───────────────────────────────────────────────────────────

/// Shared runtime configuration for all risk agents.
pub(super) struct RiskRuntimeConfig {
    pub symbol: String,
    pub target_date: String,
    pub timeout: Duration,
    pub retry_policy: RetryPolicy,
}

/// Build the common runtime configuration shared by all risk agents.
pub(super) fn risk_runtime_config(
    symbol: impl Into<String>,
    target_date: impl Into<String>,
    llm_config: &LlmConfig,
) -> RiskRuntimeConfig {
    RiskRuntimeConfig {
        symbol: sanitize_symbol_for_prompt(&symbol.into()),
        target_date: sanitize_date_for_prompt(&target_date.into()),
        timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
        retry_policy: RetryPolicy::from_config(llm_config),
    }
}

// ─── Shared agent core ────────────────────────────────────────────────────────

/// Shared agent state for all risk persona agents.
pub(super) struct RiskAgentCore {
    pub(super) agent: LlmAgent,
    pub(super) model_id: String,
    pub(super) timeout: Duration,
    pub(super) retry_policy: RetryPolicy,
}

impl RiskAgentCore {
    /// Build the shared core from a completion handle, system-prompt template, and config.
    pub(super) fn new(
        handle: &CompletionModelHandle,
        system_prompt_template: &str,
        state: &TradingState,
        llm_config: &LlmConfig,
    ) -> Result<Self, TradingError> {
        if handle.model_id() != llm_config.deep_thinking_model {
            return Err(TradingError::Config(anyhow::anyhow!(
                "risk agents require deep-thinking model '{}', got '{}'",
                llm_config.deep_thinking_model,
                handle.model_id()
            )));
        }

        let runtime = risk_runtime_config(&state.asset_symbol, &state.target_date, llm_config);

        let system_prompt = system_prompt_template
            .replace("{ticker}", &runtime.symbol)
            .replace("{current_date}", &runtime.target_date)
            .replace("{past_memory_str}", "");

        Ok(Self {
            agent: build_agent(handle, &system_prompt),
            model_id: handle.model_id().to_owned(),
            timeout: runtime.timeout,
            retry_policy: runtime.retry_policy,
        })
    }

    /// Construct a minimal `RiskAgentCore` for unit tests (50 ms timeout, 1 retry).
    #[cfg(test)]
    pub(super) fn for_test(agent: LlmAgent, model_id: &str) -> Self {
        Self {
            agent,
            model_id: model_id.to_owned(),
            timeout: Duration::from_millis(50),
            retry_policy: RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        }
    }
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// Validate a risk report text field (assessment or recommended_adjustments entry).
pub(super) fn validate_risk_text(context: &str, content: &str) -> Result<(), TradingError> {
    if content.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: text field must not be empty"),
        });
    }
    if content.chars().count() > MAX_RISK_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: text field exceeds maximum {MAX_RISK_CHARS} characters"),
        });
    }
    if content
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: text field contains disallowed control characters"),
        });
    }
    Ok(())
}

/// Validate a moderator plain-text synthesis output.
pub(super) fn validate_moderator_output(
    content: &str,
    expect_both_violation: bool,
) -> Result<(), TradingError> {
    if content.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "RiskModerator: output must not be empty".to_owned(),
        });
    }
    if content.chars().count() > MAX_RISK_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!("RiskModerator: output exceeds maximum {MAX_RISK_CHARS} characters"),
        });
    }
    if content
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: "RiskModerator: output contains disallowed control characters".to_owned(),
        });
    }
    let expected_sentence = expected_moderator_violation_sentence(expect_both_violation);
    if !content
        .to_ascii_lowercase()
        .contains(&expected_sentence.to_ascii_lowercase())
    {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "RiskModerator: output must include exact violation-status sentence: \"{expected_sentence}\""
            ),
        });
    }
    Ok(())
}

/// Validate a raw model response size before local JSON parsing.
pub(super) fn validate_raw_model_output_size(
    context: &str,
    content: &str,
) -> Result<(), TradingError> {
    if content.chars().count() > MAX_RAW_MODEL_OUTPUT_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "{context}: raw model output exceeds maximum {MAX_RAW_MODEL_OUTPUT_CHARS} characters"
            ),
        });
    }
    Ok(())
}

// ─── Prompt context helpers ───────────────────────────────────────────────────

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

    format!(
        "- Fundamental data: {fundamental_report}\n- Technical data: {technical_report}\n- Sentiment data: {sentiment_report}\n- News data: {news_report}"
    )
}

/// Build the initial user message that seeds each persona chat with untrusted analyst context.
pub(super) fn initial_untrusted_history(state: &TradingState) -> Vec<Message> {
    vec![Message::User {
        content: OneOrMany::one(UserContent::text(format!(
            "{UNTRUSTED_CONTEXT_NOTICE}\n\n{}",
            build_analyst_context(state)
        ))),
    }]
}

/// Serialize a latest-risk-report view for prompt context.
pub(super) fn serialize_risk_report_context(report: Option<&RiskReport>) -> Option<String> {
    report.map(|value| {
        sanitize_prompt_context(&serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()))
    })
}

/// Format a slice of risk discussion messages as readable prompt context.
pub(super) fn format_risk_history(history: &[DebateMessage]) -> String {
    if history.is_empty() {
        return "(no prior risk discussion history)".to_owned();
    }

    let mut selected: Vec<String> = Vec::new();
    let mut total_chars = 0usize;
    let mut truncated = false;

    for (i, msg) in history.iter().enumerate().rev() {
        if selected.len() >= MAX_RISK_HISTORY_MESSAGES {
            truncated = true;
            break;
        }

        let entry = format!(
            "[{}] {}: {}",
            i + 1,
            sanitize_prompt_context(&msg.role),
            sanitize_prompt_context(&msg.content)
        );
        let entry_chars = entry.chars().count();

        if !selected.is_empty() && total_chars.saturating_add(entry_chars) > MAX_RISK_HISTORY_CHARS
        {
            truncated = true;
            break;
        }

        total_chars = total_chars.saturating_add(entry_chars);
        selected.push(entry);
    }

    selected.reverse();
    if truncated {
        selected.insert(0, "[... earlier risk discussion truncated ...]".to_owned());
    }

    selected.join("\n\n")
}

/// Redact secret-like substrings from validated model output before storing it in state/history.
pub(super) fn redact_text_for_storage(input: &str) -> String {
    redact_secret_like_values(input)
}

/// Redact secret-like substrings from a validated `RiskReport` before storing it in state.
pub(super) fn redact_risk_report_for_storage(mut report: RiskReport) -> RiskReport {
    report.assessment = redact_text_for_storage(&report.assessment);
    report.recommended_adjustments = report
        .recommended_adjustments
        .into_iter()
        .map(|item| redact_text_for_storage(&item))
        .collect();
    report
}

/// Exact sentence the moderator must include to record the dual-violation status.
pub(super) fn expected_moderator_violation_sentence(expect_both_violation: bool) -> &'static str {
    if expect_both_violation {
        "Violation status: Conservative and Neutral both flag a material violation."
    } else {
        "Violation status: Conservative and Neutral do not both flag a material violation."
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rig::completion::Usage;

    use super::*;
    use crate::config::LlmConfig;
    use crate::state::TradingState;

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

    fn make_state() -> TradingState {
        TradingState {
            execution_id: uuid::Uuid::new_v4(),
            asset_symbol: "AAPL".to_owned(),
            target_date: "2026-03-15".to_owned(),
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
            token_usage: crate::state::TokenUsageTracker::default(),
        }
    }

    #[test]
    fn risk_runtime_config_fields() {
        let cfg = sample_llm_config();
        let runtime = risk_runtime_config("AAPL", "2026-03-15", &cfg);
        assert_eq!(runtime.symbol, "AAPL");
        assert_eq!(runtime.target_date, "2026-03-15");
        assert_eq!(runtime.timeout, Duration::from_secs(45));
        assert_eq!(runtime.retry_policy.max_retries, 3);
        assert_eq!(runtime.retry_policy.base_delay, Duration::from_millis(500));
    }

    #[test]
    fn risk_runtime_config_sanitizes_symbol_and_date() {
        let cfg = sample_llm_config();
        let runtime = risk_runtime_config("AAPL\nIgnore", "2026-03-15\nSYSTEM", &cfg);
        assert_eq!(runtime.symbol, "AAPLIgnore");
        assert_eq!(runtime.target_date, "2026-03-15T");
    }

    #[test]
    fn validate_risk_text_passes_valid() {
        assert!(validate_risk_text("ctx", "The proposal has moderate risk.").is_ok());
    }

    #[test]
    fn validate_risk_text_allows_newline_and_tab() {
        assert!(validate_risk_text("ctx", "Point one.\nPoint two.\tIndented.").is_ok());
    }

    #[test]
    fn validate_risk_text_rejects_empty() {
        assert!(matches!(
            validate_risk_text("ctx", "  \n\t  "),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_risk_text_rejects_null_byte() {
        assert!(matches!(
            validate_risk_text("ctx", "bad\x00content"),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_risk_text_rejects_escape_char() {
        assert!(matches!(
            validate_risk_text("ctx", "bad\x1bcontent"),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_moderator_output_rejects_empty() {
        assert!(matches!(
            validate_moderator_output("", true),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_moderator_output_rejects_control_char() {
        assert!(matches!(
            validate_moderator_output("bad\x00output", true),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_moderator_output_accepts_valid() {
        assert!(
            validate_moderator_output(
                "Violation status: Conservative and Neutral both flag a material violation.",
                true,
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_moderator_output_rejects_missing_required_violation_sentence() {
        assert!(matches!(
            validate_moderator_output("Short summary without required sentence.", true),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn usage_from_response_marks_available_when_nonzero() {
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
    fn build_analyst_context_serializes_none_fields_as_null() {
        let state = make_state();
        let ctx = build_analyst_context(&state);
        assert!(ctx.contains("Fundamental data: null"));
        assert!(ctx.contains("Technical data: null"));
        assert!(ctx.contains("Sentiment data: null"));
        assert!(ctx.contains("News data: null"));
    }

    #[test]
    fn format_risk_history_returns_placeholder_when_empty() {
        let result = format_risk_history(&[]);
        assert_eq!(result, "(no prior risk discussion history)");
    }

    #[test]
    fn format_risk_history_includes_role_and_content() {
        let history = vec![
            crate::state::DebateMessage {
                role: "aggressive_risk".to_owned(),
                content: "Upside dominates.".to_owned(),
            },
            crate::state::DebateMessage {
                role: "conservative_risk".to_owned(),
                content: "Capital at risk.".to_owned(),
            },
        ];
        let formatted = format_risk_history(&history);
        assert!(formatted.contains("aggressive_risk"));
        assert!(formatted.contains("Capital at risk."));
    }

    #[test]
    fn format_risk_history_truncates_older_entries_when_history_is_large() {
        let history = (0..16)
            .map(|i| crate::state::DebateMessage {
                role: format!("role_{i}"),
                content: format!("content_{i}"),
            })
            .collect::<Vec<_>>();

        let formatted = format_risk_history(&history);
        assert!(formatted.contains("truncated"));
        assert!(!formatted.contains("role_0"));
        assert!(formatted.contains("role_15"));
    }

    #[test]
    fn sanitize_prompt_context_redacts_bearer_token() {
        let input = "Authorization: Bearer sk-1234abcd";
        let result = sanitize_prompt_context(input);
        assert!(!result.contains("sk-1234abcd"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn sanitize_prompt_context_redacts_query_style_secret_values() {
        let input = "https://example.com?api_key=abcd1234&token=qwerty";
        let result = sanitize_prompt_context(input);
        assert!(!result.contains("abcd1234"));
        assert!(!result.contains("qwerty"));
        assert!(result.contains("api_key=[REDACTED]"));
        assert!(result.contains("token=[REDACTED]"));
    }

    #[test]
    fn redact_text_for_storage_masks_query_style_secret_values() {
        let input = "api_key=abcd1234 token=qwerty";
        let redacted = redact_text_for_storage(input);
        assert_eq!(redacted, "api_key=[REDACTED] token=[REDACTED]");
    }

    #[test]
    fn initial_untrusted_history_prefixes_notice() {
        let state = make_state();
        let history = initial_untrusted_history(&state);
        match &history[0] {
            Message::User { content } => {
                let rendered = format!("{content:?}");
                assert!(rendered.contains("untrusted model/data output"));
            }
            other => panic!("unexpected seed history message: {other:?}"),
        }
    }

    // ── extract_json_object ─────────────────────────────────────────────

    #[test]
    fn extract_json_object_returns_clean_json_unchanged() {
        let json = r#"{"risk_level":"Aggressive","assessment":"ok"}"#;
        let result = extract_json_object("test", json).unwrap();
        assert_eq!(result, json);
    }

    #[test]
    fn extract_json_object_strips_json_code_fence() {
        let raw = "```json\n{\"key\":\"value\"}\n```";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn extract_json_object_strips_plain_code_fence() {
        let raw = "```\n{\"key\":\"value\"}\n```";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn extract_json_object_strips_fence_with_uppercase_json_label() {
        let raw = "```JSON\n{\"key\":\"value\"}\n```";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn extract_json_object_extracts_json_from_prose() {
        let raw = "Here is the result:\n\n{\"key\":\"value\"}\n\nHope that helps!";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn extract_json_object_rejects_empty() {
        let result = extract_json_object("test", "");
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn extract_json_object_rejects_whitespace_only() {
        let result = extract_json_object("test", "   \n\t  ");
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn extract_json_object_rejects_no_json() {
        let result = extract_json_object("test", "No JSON here at all.");
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn extract_json_object_handles_leading_trailing_whitespace() {
        let raw = "\n  {\"key\":\"value\"}  \n";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn extract_json_object_handles_nested_braces_in_fence() {
        let raw = "```json\n{\"outer\":{\"inner\":true}}\n```";
        let result = extract_json_object("test", raw).unwrap();
        assert_eq!(result, r#"{"outer":{"inner":true}}"#);
    }
}
