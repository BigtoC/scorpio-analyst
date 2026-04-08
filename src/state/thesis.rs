//! Thesis-memory payload for cross-run context continuity.
//!
//! A [`ThesisMemory`] captures the authoritative trading thesis from a completed
//! run so a subsequent run for the same symbol can load and inject it as
//! historical context into downstream prompts.
//!
//! The payload is intentionally compact and typed. Downstream prompt helpers
//! frame it as "historical context for reference" rather than an authoritative
//! conclusion to guard against positive-feedback loops.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Compact thesis-memory payload captured at the end of a completed run.
///
/// Stored on the phase-5 snapshot row via `TradingState::current_thesis` and
/// retrieved by future runs via `SnapshotStore::load_prior_thesis_for_symbol`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThesisMemory {
    /// Canonical asset symbol (matches `TradingState::asset_symbol` after preflight).
    pub symbol: String,

    /// Trade action from the final run: `"Buy"`, `"Sell"`, or `"Hold"`.
    pub action: String,

    /// Final fund manager decision: `"Approved"` or `"Rejected"`.
    pub decision: String,

    /// Rationale that drove the final decision.
    ///
    /// Kept at most [`crate::constants::MAX_PROMPT_CONTEXT_CHARS`] characters long
    /// by the prompt helper at injection time.
    pub rationale: String,

    /// Optional one-line summary for compact display.
    #[serde(default)]
    pub summary: Option<String>,

    /// Execution ID (UUID string) of the run that produced this thesis.
    pub execution_id: String,

    /// Analysis target date of the originating run (e.g. `"2026-04-07"`).
    pub target_date: String,

    /// UTC timestamp when this thesis was captured.
    pub captured_at: DateTime<Utc>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::ThesisMemory;

    fn sample_thesis() -> ThesisMemory {
        ThesisMemory {
            symbol: "AAPL".to_owned(),
            action: "Buy".to_owned(),
            decision: "Approved".to_owned(),
            rationale: "Strong fundamentals support upside with limited downside risk.".to_owned(),
            summary: Some("Buy: strong fundamentals".to_owned()),
            execution_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            target_date: "2026-04-07".to_owned(),
            captured_at: Utc::now(),
        }
    }

    #[test]
    fn thesis_memory_serde_round_trip() {
        let thesis = sample_thesis();
        let json = serde_json::to_string(&thesis).expect("serialization should succeed");
        let recovered: ThesisMemory =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(thesis.symbol, recovered.symbol);
        assert_eq!(thesis.action, recovered.action);
        assert_eq!(thesis.decision, recovered.decision);
        assert_eq!(thesis.rationale, recovered.rationale);
        assert_eq!(thesis.summary, recovered.summary);
        assert_eq!(thesis.execution_id, recovered.execution_id);
        assert_eq!(thesis.target_date, recovered.target_date);
    }

    #[test]
    fn thesis_memory_serde_tolerates_missing_optional_summary() {
        let json = r#"{
            "symbol": "TSLA",
            "action": "Hold",
            "decision": "Rejected",
            "rationale": "Valuation stretched.",
            "execution_id": "abc123",
            "target_date": "2026-01-01",
            "captured_at": "2026-01-01T10:00:00Z"
        }"#;
        let thesis: ThesisMemory = serde_json::from_str(json)
            .expect("missing 'summary' field should deserialize with default None");
        assert!(thesis.summary.is_none());
        assert_eq!(thesis.symbol, "TSLA");
        assert_eq!(thesis.action, "Hold");
    }

    #[test]
    fn thesis_memory_serde_preserves_all_optional_fields_when_present() {
        let thesis = sample_thesis();
        let json = serde_json::to_string(&thesis).unwrap();
        let recovered: ThesisMemory = serde_json::from_str(&json).unwrap();
        assert!(recovered.summary.is_some());
        assert_eq!(
            recovered.summary.as_deref(),
            Some("Buy: strong fundamentals")
        );
    }

    #[test]
    fn thesis_memory_implements_debug() {
        let thesis = sample_thesis();
        let rendered = format!("{thesis:?}");
        assert!(rendered.contains("ThesisMemory"));
        assert!(rendered.contains("AAPL"));
    }
}
