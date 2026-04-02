use serde::Deserialize;

use crate::{
    agents::risk::extract_json_object,
    constants::{MAX_RATIONALE_CHARS, MAX_RAW_RESPONSE_CHARS},
    error::TradingError,
    state::{Decision, ExecutionStatus, TradingState},
};

pub(super) const DETERMINISTIC_REJECT_RATIONALE: &str = "Both the Conservative and Neutral risk reports flag a material violation \
     (flags_violation == true). Proposal rejected by deterministic safety-net \
     without LLM consultation.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionStatusResponse {
    decision: Decision,
    rationale: String,
    decided_at: Option<String>,
}

pub(super) fn parse_and_validate_execution_status(
    raw_output: &str,
    requires_missing_data_acknowledgment: bool,
    target_date: &str,
) -> Result<ExecutionStatus, TradingError> {
    let raw_char_count = raw_output.chars().count();
    if raw_char_count > MAX_RAW_RESPONSE_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "FundManager: response exceeds maximum {} characters",
                MAX_RAW_RESPONSE_CHARS
            ),
        });
    }

    let cleaned = extract_json_object("FundManager", raw_output)?;
    let parsed: ExecutionStatusResponse =
        serde_json::from_str(&cleaned).map_err(|_| TradingError::SchemaViolation {
            message: "FundManager: response could not be parsed as ExecutionStatus".to_owned(),
        })?;

    let mut status = ExecutionStatus {
        decision: parsed.decision,
        rationale: parsed.rationale,
        decided_at: parsed.decided_at.unwrap_or_else(|| target_date.to_owned()),
    };

    validate_execution_status(&status)?;

    if requires_missing_data_acknowledgment
        && !rationale_acknowledges_missing_data(&status.rationale)
    {
        return Err(TradingError::SchemaViolation {
            message: "FundManager: rationale must acknowledge missing upstream data".to_owned(),
        });
    }

    if status.decided_at.trim().is_empty() {
        status.decided_at = target_date.to_owned();
    }

    Ok(status)
}

/// Domain-validate an [`ExecutionStatus`] after successful JSON deserialization.
///
/// All failures return [`TradingError::SchemaViolation`].
fn validate_execution_status(status: &ExecutionStatus) -> Result<(), TradingError> {
    if status.rationale.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "FundManager: rationale must not be empty".to_owned(),
        });
    }
    if status.rationale.chars().count() > MAX_RATIONALE_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "FundManager: rationale exceeds maximum {} characters",
                MAX_RATIONALE_CHARS
            ),
        });
    }
    if status
        .rationale
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: "FundManager: rationale contains disallowed control characters".to_owned(),
        });
    }
    Ok(())
}

pub(super) fn state_has_missing_analyst_inputs(state: &TradingState) -> bool {
    state.fundamental_metrics.is_none()
        || state.technical_indicators.is_none()
        || state.market_sentiment.is_none()
        || state.macro_news.is_none()
}

pub(super) fn state_has_missing_risk_reports(state: &TradingState) -> bool {
    state.aggressive_risk_report.is_none()
        || state.neutral_risk_report.is_none()
        || state.conservative_risk_report.is_none()
}

pub(super) fn state_has_missing_inputs(state: &TradingState) -> bool {
    state_has_missing_analyst_inputs(state) || state_has_missing_risk_reports(state)
}

pub(super) fn deterministic_reject(state: &TradingState) -> bool {
    let conservative_violation = state
        .conservative_risk_report
        .as_ref()
        .is_some_and(|report| report.flags_violation);
    let neutral_violation = state
        .neutral_risk_report
        .as_ref()
        .is_some_and(|report| report.flags_violation);
    conservative_violation && neutral_violation
}

/// Return the runtime-authoritative decision timestamp as an RFC 3339 / ISO 8601 string.
///
/// `Utc::now()` is infallible; `fallback` (typically `state.target_date`) is accepted
/// for API symmetry but is never reached.
pub(super) fn runtime_timestamp(_fallback: &str) -> String {
    chrono::Utc::now().to_rfc3339()
}

fn rationale_acknowledges_missing_data(rationale: &str) -> bool {
    let lowered = rationale.to_ascii_lowercase();
    ["missing", "unavailable", "partial", "gap", "upstream"]
        .iter()
        .any(|needle| lowered.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::validate_execution_status;
    use crate::{
        error::TradingError,
        state::{Decision, ExecutionStatus},
    };

    #[test]
    fn validate_rejects_empty_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: String::new(),
            decided_at: "2026-03-15".to_owned(),
        };
        assert!(matches!(
            validate_execution_status(&status),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_rejects_whitespace_only_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: "   ".to_owned(),
            decided_at: "2026-03-15".to_owned(),
        };
        assert!(matches!(
            validate_execution_status(&status),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_rejects_control_char_in_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: "bad\x00content".to_owned(),
            decided_at: "2026-03-15".to_owned(),
        };
        assert!(matches!(
            validate_execution_status(&status),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_rejects_escape_char_in_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: "bad\x1bcontent".to_owned(),
            decided_at: "2026-03-15".to_owned(),
        };
        assert!(matches!(
            validate_execution_status(&status),
            Err(TradingError::SchemaViolation { .. })
        ));
    }

    #[test]
    fn validate_allows_newline_and_tab_in_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: "Approved.\nRisk:\tWithin bounds.".to_owned(),
            decided_at: "2026-03-15".to_owned(),
        };
        assert!(validate_execution_status(&status).is_ok());
    }

    #[test]
    fn valid_approved_status_passes_validation() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            rationale: "The proposal is well-supported by all available evidence.".to_owned(),
            decided_at: "2026-03-15T00:00:00Z".to_owned(),
        };
        assert!(validate_execution_status(&status).is_ok());
    }

    #[test]
    fn valid_rejected_status_passes_validation() {
        let status = ExecutionStatus {
            decision: Decision::Rejected,
            rationale: "The stop-loss is too wide relative to the evidence quality.".to_owned(),
            decided_at: "2026-03-15T00:00:00Z".to_owned(),
        };
        assert!(validate_execution_status(&status).is_ok());
    }
}
