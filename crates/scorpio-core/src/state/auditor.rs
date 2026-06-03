use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    /// Auditor feature not enabled for this run.
    #[default]
    Disabled,
    /// Auditor is enabled for the run but has not executed yet.
    Pending,
    /// Auditor ran and produced no findings at all.
    Passed,
    /// Auditor ran and attached one or more findings.
    Findings,
    /// Auditor was enabled but failed open; the final recommendation still stands.
    FailedOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The proposal contradicts source data or contains a math error that invalidates the recommendation.
    Critical,
    /// Risky pattern (unsourced numeric claim, weak rationale, terminal-value heavy DCF, etc.) — proposal can stand but reviewer should be aware.
    Warning,
    /// Style or completeness note. Surfaced in verbose mode only.
    Info,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    pub severity: Severity,
    /// Where in `TradingState` the issue was detected. Free-form but conventionally one of:
    /// "trader_proposal.rationale", "trader_proposal.target_price",
    /// "fundamental_metrics.summary", "debate_history[12].content", etc.
    #[schemars(length(max = 128))]
    pub location: String,
    /// One-sentence description of the issue.
    #[schemars(length(max = 512))]
    pub description: String,
    /// Optional verbatim excerpt from the offending section to anchor the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 512))]
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AuditorReport {
    /// Bounded to 20 to prevent runaway output.
    #[schemars(length(max = 20))]
    pub findings: Vec<Finding>,
    /// Auditor's one-paragraph summary.
    #[schemars(length(max = 1024))]
    pub summary: String,
    /// Runtime-populated metadata; never trusted from model output.
    pub audited_at: DateTime<Utc>,
    pub auditor_model_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_serde_roundtrip() {
        let report = AuditorReport {
            findings: vec![Finding {
                severity: Severity::Critical,
                location: "trader_proposal.target_price".into(),
                description: "Target below current price for BUY".into(),
                excerpt: Some("target_price=100, current=120".into()),
            }],
            summary: "blocking issue".into(),
            audited_at: chrono::Utc::now(),
            auditor_model_id: "claude-haiku-4-5".into(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: AuditorReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn audit_status_defaults_to_disabled() {
        assert_eq!(AuditStatus::default(), AuditStatus::Disabled);
    }
}
