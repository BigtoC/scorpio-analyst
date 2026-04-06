//! Run-level coverage and provenance reporting types.
//!
//! Defines [`DataCoverageReport`] (which inputs were present vs. missing) and
//! [`ProvenanceSummary`] (which providers contributed evidence that was actually
//! used in the current run).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Run-level data coverage report derived from the typed `evidence_*` fields
/// on [`super::TradingState`].
///
/// # Coverage authority rule
///
/// `DataCoverageReport` is derived from the new `evidence_*` fields only, not
/// from the legacy analyst mirrors. `required_inputs` uses the fixed order
/// `["fundamentals", "sentiment", "news", "technical"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DataCoverageReport {
    /// The ordered list of required evidence inputs for a pipeline run.
    ///
    /// Fixed Stage 1 order: `["fundamentals", "sentiment", "news", "technical"]`.
    pub required_inputs: Vec<String>,

    /// The subset of `required_inputs` that were absent on the continue path.
    ///
    /// An empty `missing_inputs` means all required inputs were present.
    pub missing_inputs: Vec<String>,
}

/// Run-level provenance summary derived from evidence records that are actually
/// present on the continue path.
///
/// # Derivation rules
///
/// `providers_used` is built from the providers attached to evidence records
/// that were actually populated after the analyst fan-out merge. It is sorted
/// ascending and deduplicated. Absent evidence must not contribute placeholder
/// providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProvenanceSummary {
    /// Sorted, deduplicated list of provider identifiers that contributed
    /// evidence in the current pipeline run.
    ///
    /// Example: `["finnhub", "fred", "yfinance"]`.
    pub providers_used: Vec<String>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_coverage_report_serde_round_trip() {
        let original = DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec![],
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let recovered: DataCoverageReport =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original.required_inputs, recovered.required_inputs);
        assert_eq!(original.missing_inputs, recovered.missing_inputs);
    }

    #[test]
    fn data_coverage_report_with_missing_inputs_serde_round_trip() {
        let original = DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec!["technical".to_owned()],
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let recovered: DataCoverageReport =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original.missing_inputs, recovered.missing_inputs);
        assert_eq!(recovered.missing_inputs, vec!["technical"]);
    }

    #[test]
    fn provenance_summary_serde_round_trip() {
        let original = ProvenanceSummary {
            providers_used: vec![
                "finnhub".to_owned(),
                "fred".to_owned(),
                "yfinance".to_owned(),
            ],
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let recovered: ProvenanceSummary =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(original.providers_used, recovered.providers_used);
    }

    #[test]
    fn provenance_summary_empty_providers_serde_round_trip() {
        let original = ProvenanceSummary {
            providers_used: vec![],
        };

        let json = serde_json::to_string(&original).unwrap();
        let recovered: ProvenanceSummary = serde_json::from_str(&json).unwrap();
        assert!(recovered.providers_used.is_empty());
    }

    #[test]
    fn data_coverage_required_inputs_fixed_order() {
        let report = DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec![],
        };
        assert_eq!(
            report.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }
}
