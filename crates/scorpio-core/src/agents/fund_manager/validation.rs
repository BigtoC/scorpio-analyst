use serde::Deserialize;

use crate::{
    agents::{risk::DualRiskStatus, shared::extract_json_object},
    constants::{MAX_RATIONALE_CHARS, MAX_RAW_RESPONSE_CHARS},
    error::TradingError,
    state::{Decision, ExecutionStatus, TradeAction, TradingState},
};

#[derive(Debug, Deserialize)]
struct ExecutionStatusResponse {
    decision: Decision,
    action: TradeAction,
    rationale: String,
    decided_at: Option<String>,
    entry_guidance: Option<String>,
    suggested_position: Option<String>,
}

pub(super) fn parse_and_validate_execution_status(
    raw_output: &str,
    requires_missing_data_acknowledgment: bool,
    target_date: &str,
    dual_risk_status: DualRiskStatus,
    trader_proposal_action: TradeAction,
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
        action: parsed.action,
        rationale: parsed.rationale,
        decided_at: parsed.decided_at.unwrap_or_else(|| target_date.to_owned()),
        entry_guidance: parsed.entry_guidance,
        suggested_position: parsed.suggested_position,
    };

    validate_execution_status(&status)?;

    if requires_missing_data_acknowledgment
        && !rationale_acknowledges_missing_data(&status.rationale)
    {
        return Err(TradingError::SchemaViolation {
            message: "FundManager: rationale must acknowledge missing upstream data".to_owned(),
        });
    }

    validate_dual_risk_rationale(&status, dual_risk_status, trader_proposal_action)?;

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
    state.fundamental_metrics().is_none()
        || state.technical_indicators().is_none()
        || state.market_sentiment().is_none()
        || state.macro_news().is_none()
}

pub(super) fn state_has_missing_risk_reports(state: &TradingState) -> bool {
    state.aggressive_risk_report.is_none()
        || state.neutral_risk_report.is_none()
        || state.conservative_risk_report.is_none()
}

pub(super) fn state_has_missing_inputs(state: &TradingState) -> bool {
    state_has_missing_analyst_inputs(state) || state_has_missing_risk_reports(state)
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

/// Return the first non-empty line of the rationale, tolerating at most one leading `\n`.
fn first_rationale_line(rationale: &str) -> Result<&str, TradingError> {
    let content = match rationale.strip_prefix('\n') {
        Some(rest) => {
            if rest.starts_with('\n') {
                return Err(TradingError::SchemaViolation {
                    message: "FundManager: rationale has more than one leading newline before the required first-line prefix".to_owned(),
                });
            }
            rest
        }
        None => rationale,
    };
    Ok(content.split('\n').next().unwrap_or(""))
}

/// Validate the first-line prefix and action-direction contract when dual-risk is relevant.
fn validate_dual_risk_rationale(
    status: &ExecutionStatus,
    dual_risk_status: DualRiskStatus,
    trader_proposal_action: TradeAction,
) -> Result<(), TradingError> {
    let first_line = first_rationale_line(&status.rationale)?;
    const ESCALATION_PREFIX: &str = "Dual-risk escalation: ";

    if dual_risk_status == DualRiskStatus::Absent && first_line.starts_with(ESCALATION_PREFIX) {
        return Err(TradingError::SchemaViolation {
            message: "FundManager: dual-risk escalation absent — rationale must not use a dual-risk escalation first-line prefix"
                .to_owned(),
        });
    }

    match dual_risk_status {
        DualRiskStatus::Absent => return Ok(()),
        DualRiskStatus::Present => {
            // Same-direction rejection is invalid (e.g. trader proposed Buy, FM rejected Buy).
            // Exception: trader proposed Hold (no defined "opposite direction").
            if status.decision == Decision::Rejected
                && status.action != TradeAction::Hold
                && trader_proposal_action != TradeAction::Hold
                && status.action == trader_proposal_action
            {
                return Err(TradingError::SchemaViolation {
                    message: format!(
                        "FundManager: dual-risk escalation present — Rejected+{:?} is invalid when trader also proposed {:?} (same-direction rejection)",
                        status.action, trader_proposal_action
                    ),
                });
            }

            let required_prefix = match (&status.decision, &status.action) {
                (Decision::Rejected, _) => "Dual-risk escalation: upheld because ",
                (Decision::Approved, TradeAction::Hold) => {
                    "Dual-risk escalation: deferred because "
                }
                (Decision::Approved, _) => "Dual-risk escalation: overridden because ",
            };

            if !first_line.starts_with(required_prefix) {
                return Err(TradingError::SchemaViolation {
                    message: format!(
                        "FundManager: dual-risk escalation present — first rationale line must start with \"{required_prefix}\""
                    ),
                });
            }
        }
        DualRiskStatus::Unknown => {
            const REQUIRED_PREFIX: &str = "Dual-risk escalation: indeterminate because ";
            if !first_line.starts_with(REQUIRED_PREFIX) {
                return Err(TradingError::SchemaViolation {
                    message: format!(
                        "FundManager: dual-risk status unknown — first rationale line must start with \"{REQUIRED_PREFIX}\""
                    ),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_execution_status;
    use crate::{
        error::TradingError,
        state::{Decision, ExecutionStatus, TradeAction},
    };

    #[test]
    fn validate_rejects_empty_rationale() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: String::new(),
            decided_at: "2026-03-15".to_owned(),
            entry_guidance: None,
            suggested_position: None,
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
            action: TradeAction::Buy,
            rationale: "   ".to_owned(),
            decided_at: "2026-03-15".to_owned(),
            entry_guidance: None,
            suggested_position: None,
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
            action: TradeAction::Buy,
            rationale: "bad\x00content".to_owned(),
            decided_at: "2026-03-15".to_owned(),
            entry_guidance: None,
            suggested_position: None,
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
            action: TradeAction::Buy,
            rationale: "bad\x1bcontent".to_owned(),
            decided_at: "2026-03-15".to_owned(),
            entry_guidance: None,
            suggested_position: None,
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
            action: TradeAction::Buy,
            rationale: "Approved.\nRisk:\tWithin bounds.".to_owned(),
            decided_at: "2026-03-15".to_owned(),
            entry_guidance: None,
            suggested_position: None,
        };
        assert!(validate_execution_status(&status).is_ok());
    }

    #[test]
    fn valid_approved_status_passes_validation() {
        let status = ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: "The proposal is well-supported by all available evidence.".to_owned(),
            decided_at: "2026-03-15T00:00:00Z".to_owned(),
            entry_guidance: None,
            suggested_position: None,
        };
        assert!(validate_execution_status(&status).is_ok());
    }

    #[test]
    fn valid_rejected_status_passes_validation() {
        let status = ExecutionStatus {
            decision: Decision::Rejected,
            action: TradeAction::Hold,
            rationale: "The stop-loss is too wide relative to the evidence quality.".to_owned(),
            decided_at: "2026-03-15T00:00:00Z".to_owned(),
            entry_guidance: None,
            suggested_position: None,
        };
        assert!(validate_execution_status(&status).is_ok());
    }
}
