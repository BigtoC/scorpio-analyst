//! Generic evidence envelope.
//!
//! Defines [`EvidenceKind`] (the category of evidence) and the generic
//! [`EvidenceRecord<T>`] wrapper that carries any analyst payload alongside
//! provenance and quality metadata.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DataQualityFlag, EvidenceSource};

/// The category of evidence captured by an [`EvidenceRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Fundamental,
    Technical,
    Sentiment,
    News,
    Macro,
    Transcript,
    Estimates,
    Peers,
    Volatility,
}

/// Generic typed evidence envelope.
///
/// `EvidenceRecord<T>` wraps any analyst payload type with common provenance
/// and quality metadata. Stage 1 always sets `quality_flags` to `[]` and
/// sources use the fixed Stage 1 provider/dataset mapping defined in
/// `AnalystSyncTask`.
///
/// The derive macros add the necessary bounds for `T` automatically:
/// `Clone`, `PartialEq`, `Serialize`, `Deserialize`, and `JsonSchema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceRecord<T> {
    /// The category of evidence this record represents.
    pub kind: EvidenceKind,

    /// The typed analyst payload.
    pub payload: T,

    /// The provider sources that contributed this evidence record.
    pub sources: Vec<EvidenceSource>,

    /// Quality caveats for this record (always empty in Stage 1).
    pub quality_flags: Vec<DataQualityFlag>,
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::state::{EvidenceSource, FundamentalData};

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

    fn sample_fundamental() -> FundamentalData {
        FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(25.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "test fundamental".to_owned(),
        }
    }

    #[test]
    fn evidence_kind_serde_round_trip() {
        for kind in [
            EvidenceKind::Fundamental,
            EvidenceKind::Technical,
            EvidenceKind::Sentiment,
            EvidenceKind::News,
            EvidenceKind::Macro,
            EvidenceKind::Transcript,
            EvidenceKind::Estimates,
            EvidenceKind::Peers,
            EvidenceKind::Volatility,
        ] {
            let json = serde_json::to_string(&kind).expect("serialization should succeed");
            let recovered: EvidenceKind =
                serde_json::from_str(&json).expect("deserialization should succeed");
            assert_eq!(kind, recovered);
        }
    }

    #[test]
    fn evidence_kind_serializes_snake_case() {
        let json = serde_json::to_string(&EvidenceKind::Fundamental).unwrap();
        assert_eq!(json, "\"fundamental\"");
        let json = serde_json::to_string(&EvidenceKind::Technical).unwrap();
        assert_eq!(json, "\"technical\"");
    }

    #[test]
    fn evidence_record_fundamental_serde_round_trip() {
        let record: EvidenceRecord<FundamentalData> = EvidenceRecord {
            kind: EvidenceKind::Fundamental,
            payload: sample_fundamental(),
            sources: vec![sample_source()],
            quality_flags: vec![],
        };

        let json = serde_json::to_string(&record).expect("serialization should succeed");
        let recovered: EvidenceRecord<FundamentalData> =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(record.kind, recovered.kind);
        assert_eq!(record.payload.pe_ratio, recovered.payload.pe_ratio);
        assert_eq!(record.sources.len(), recovered.sources.len());
        assert_eq!(record.quality_flags, recovered.quality_flags);
    }

    #[test]
    fn evidence_record_json_value_serde_round_trip() {
        let payload = serde_json::json!({"key": "value", "number": 42});
        let record: EvidenceRecord<serde_json::Value> = EvidenceRecord {
            kind: EvidenceKind::News,
            payload,
            sources: vec![],
            quality_flags: vec![],
        };

        let json = serde_json::to_string(&record).expect("serialization should succeed");
        let recovered: EvidenceRecord<serde_json::Value> =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(record.kind, recovered.kind);
        assert_eq!(record.payload, recovered.payload);
    }

    #[test]
    fn evidence_record_quality_flags_empty_in_stage1() {
        let record: EvidenceRecord<FundamentalData> = EvidenceRecord {
            kind: EvidenceKind::Fundamental,
            payload: sample_fundamental(),
            sources: vec![],
            quality_flags: vec![],
        };
        assert!(
            record.quality_flags.is_empty(),
            "Stage 1 evidence records must have empty quality_flags"
        );
    }
}
