//! Shared helpers for analyst agents.
//!
//! Extracted here to avoid verbatim duplication across the four analyst
//! modules (`fundamental`, `sentiment`, `news`, `technical`).

use std::time::Instant;

use crate::{error::TradingError, state::AgentTokenUsage};

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
