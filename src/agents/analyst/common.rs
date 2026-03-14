//! Shared helpers for analyst agents.
//!
//! Extracted here to avoid verbatim duplication across the four analyst
//! modules (`fundamental`, `sentiment`, `news`, `technical`).

use std::time::Duration;
use std::time::Instant;

use crate::{
    config::LlmConfig,
    error::{RetryPolicy, TradingError},
    state::AgentTokenUsage,
};

/// Shared runtime fields derived from the analyst request context.
///
/// Keeping these fields together removes duplicated constructor code while
/// preserving explicit, agent-specific constructors in each analyst module.
pub(super) struct AnalystRuntimeConfig {
    pub symbol: String,
    pub target_date: String,
    pub timeout: Duration,
    pub retry_policy: RetryPolicy,
}

/// Build the common runtime configuration shared by all analyst agents.
pub(super) fn analyst_runtime_config(
    symbol: impl Into<String>,
    target_date: impl Into<String>,
    llm_config: &LlmConfig,
) -> AnalystRuntimeConfig {
    AnalystRuntimeConfig {
        symbol: symbol.into(),
        target_date: target_date.into(),
        timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
        retry_policy: RetryPolicy::from_config(llm_config),
    }
}

/// Maximum characters allowed in any LLM-generated summary field.
///
/// Prevents adversarial overflow of downstream state buffers and limits the
/// extent of prompt-injection content that can propagate through phases.
pub(super) const MAX_SUMMARY_CHARS: usize = 4_096;

/// Validate that a summary is within length bounds and free of control characters.
pub(super) fn validate_summary_content(context: &str, summary: &str) -> Result<(), TradingError> {
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: summary exceeds maximum {MAX_SUMMARY_CHARS} characters"),
        });
    }
    if summary
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: summary contains disallowed control characters"),
        });
    }
    Ok(())
}

/// Build an [`AgentTokenUsage`] from a `rig` usage response.
pub(super) fn usage_from_response(
    agent_name: &str,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
) -> AgentTokenUsage {
    AgentTokenUsage {
        agent_name: agent_name.to_owned(),
        model_id: model_id.to_owned(),
        token_counts_available: usage.total_tokens > 0
            || usage.input_tokens > 0
            || usage.output_tokens > 0,
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        latency_ms: started_at.elapsed().as_millis() as u64,
    }
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
            retry_max_retries: 4,
            retry_base_delay_ms: 750,
        }
    }

    // ── analyst_runtime_config ───────────────────────────────────────────

    #[test]
    fn analyst_runtime_config_uses_symbol_target_and_retry_settings() {
        let llm_config = sample_llm_config();

        let runtime = analyst_runtime_config("AAPL", "2026-03-14", &llm_config);

        assert_eq!(runtime.symbol, "AAPL");
        assert_eq!(runtime.target_date, "2026-03-14");
        assert_eq!(runtime.timeout, Duration::from_secs(45));
        assert_eq!(runtime.retry_policy.max_retries, 4);
        assert_eq!(runtime.retry_policy.base_delay, Duration::from_millis(750));
    }

    // ── validate_summary_content ─────────────────────────────────────────

    // TC-5: baseline — valid input passes
    #[test]
    fn validate_summary_content_passes_for_valid_input() {
        assert!(validate_summary_content("ctx", "A well-formed summary.").is_ok());
    }

    // TC-5: newline and tab are allowed control characters
    #[test]
    fn validate_summary_content_newline_and_tab_are_allowed() {
        let summary = "Line one.\nLine two.\tTabbed.";
        assert!(
            validate_summary_content("ctx", summary).is_ok(),
            "\\n and \\t should be allowed"
        );
    }

    // TC-6: summary exceeding MAX_SUMMARY_CHARS returns SchemaViolation
    #[test]
    fn validate_summary_content_too_long_returns_schema_violation() {
        let long_summary = "a".repeat(MAX_SUMMARY_CHARS + 1);
        let result = validate_summary_content("ctx", &long_summary);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // TC-6 inverse: summary at exactly MAX_SUMMARY_CHARS is accepted
    #[test]
    fn validate_summary_content_at_exact_limit_is_valid() {
        let exact_summary = "b".repeat(MAX_SUMMARY_CHARS);
        assert!(validate_summary_content("ctx", &exact_summary).is_ok());
    }

    // TC-7: summary containing a NUL control character returns SchemaViolation
    #[test]
    fn validate_summary_content_nul_control_char_returns_schema_violation() {
        let result = validate_summary_content("ctx", "bad\x00content");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // TC-7: ESC control character also rejected
    #[test]
    fn validate_summary_content_escape_control_char_returns_schema_violation() {
        let result = validate_summary_content("ctx", "bad\x1bcontent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── usage_from_response ──────────────────────────────────────────────

    // TC-8: token_counts_available = true when total_tokens > 0
    #[test]
    fn usage_from_response_marks_available_when_total_nonzero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 150,
            cached_input_tokens: 0,
        };
        let result = usage_from_response("Agent", "model-x", usage, Instant::now());
        assert!(
            result.token_counts_available,
            "should be available when total_tokens > 0"
        );
        assert_eq!(result.total_tokens, 150);
    }

    // TC-8: token_counts_available = true when input_tokens > 0 (total may be 0)
    #[test]
    fn usage_from_response_marks_available_when_input_nonzero() {
        let usage = Usage {
            input_tokens: 80,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
        };
        let result = usage_from_response("Agent", "model-x", usage, Instant::now());
        assert!(
            result.token_counts_available,
            "should be available when input_tokens > 0"
        );
        assert_eq!(result.prompt_tokens, 80);
    }

    // TC-8: token_counts_available = false when all counts are zero
    #[test]
    fn usage_from_response_marks_unavailable_when_all_zero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
        };
        let result = usage_from_response("Agent", "model-x", usage, Instant::now());
        assert!(
            !result.token_counts_available,
            "should be unavailable when all token counts are zero"
        );
    }

    // Fields are copied correctly
    #[test]
    fn usage_from_response_copies_fields() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cached_input_tokens: 0,
        };
        let result = usage_from_response("MyAgent", "my-model", usage, Instant::now());
        assert_eq!(result.agent_name, "MyAgent");
        assert_eq!(result.model_id, "my-model");
        assert_eq!(result.prompt_tokens, 100);
        assert_eq!(result.completion_tokens, 50);
        assert_eq!(result.total_tokens, 150);
    }
}
