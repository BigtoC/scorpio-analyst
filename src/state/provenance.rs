//! Evidence provenance primitives.
//!
//! Defines [`EvidenceSource`] (the provider that contributed a piece of
//! evidence) and [`DataQualityFlag`] (quality caveats that can be attached to
//! an [`super::evidence::EvidenceRecord`]).
//!
//! Stage 1 constraints:
//! - `EvidenceSource.effective_at`, `.url`, and `.citation` are always `None`.
//! - `DataQualityFlag::Conflicted` must not be emitted.
//! - `quality_flags` vectors on evidence records are always empty.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The provider that contributed a piece of evidence and the dataset(s) it
/// supplied in the current pipeline run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceSource {
    /// Provider identifier, e.g. `"finnhub"`, `"fred"`, `"yfinance"`.
    pub provider: String,

    /// Dataset(s) contributed by this provider for this evidence record.
    ///
    /// Examples: `["fundamentals"]`, `["company_news", "macro_indicators"]`.
    pub datasets: Vec<String>,

    /// Wall-clock time at which this evidence was fetched into the pipeline.
    pub fetched_at: DateTime<Utc>,

    /// Effective data date reported by the source (None in Stage 1).
    pub effective_at: Option<DateTime<Utc>>,

    /// Source URL for citation (None in Stage 1).
    pub url: Option<String>,

    /// Human-readable citation string (None in Stage 1).
    pub citation: Option<String>,
}

/// Data quality caveats that can be attached to an evidence record.
///
/// Stage 1 always leaves `quality_flags` empty; `Conflicted` must not be
/// emitted until a later milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataQualityFlag {
    /// Evidence from multiple providers conflicts and cannot be reconciled.
    ///
    /// Must not be emitted in Stage 1.
    Conflicted,

    /// Evidence is stale beyond the expected freshness window.
    Stale,

    /// Evidence has an incomplete or missing value set.
    Incomplete,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn sample_source() -> EvidenceSource {
        EvidenceSource {
            provider: "finnhub".to_owned(),
            datasets: vec!["fundamentals".to_owned()],
            fetched_at: Utc::now(),
            effective_at: None,
            url: None,
            citation: None,
        }
    }

    #[test]
    fn evidence_source_serde_round_trip() {
        let original = sample_source();
        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let recovered: EvidenceSource =
            serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(original.provider, recovered.provider);
        assert_eq!(original.datasets, recovered.datasets);
        assert_eq!(original.effective_at, recovered.effective_at);
        assert_eq!(original.url, recovered.url);
        assert_eq!(original.citation, recovered.citation);
    }

    #[test]
    fn evidence_source_optional_fields_serialize_as_null() {
        let source = sample_source();
        let json = serde_json::to_string(&source).expect("serialization should succeed");
        assert!(
            json.contains("null"),
            "optional None fields should serialize as null"
        );
    }

    #[test]
    fn data_quality_flag_serde_round_trip() {
        for flag in [
            DataQualityFlag::Conflicted,
            DataQualityFlag::Stale,
            DataQualityFlag::Incomplete,
        ] {
            let json = serde_json::to_string(&flag).expect("serialization should succeed");
            let recovered: DataQualityFlag =
                serde_json::from_str(&json).expect("deserialization should succeed");
            assert_eq!(flag, recovered);
        }
    }

    #[test]
    fn data_quality_flag_serializes_snake_case() {
        let json = serde_json::to_string(&DataQualityFlag::Conflicted).unwrap();
        assert_eq!(json, "\"conflicted\"");
        let json = serde_json::to_string(&DataQualityFlag::Stale).unwrap();
        assert_eq!(json, "\"stale\"");
        let json = serde_json::to_string(&DataQualityFlag::Incomplete).unwrap();
        assert_eq!(json, "\"incomplete\"");
    }
}
