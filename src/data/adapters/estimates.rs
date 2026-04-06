//! Stage 1 consensus-estimates evidence contract.
//!
//! Declares the [`ConsensusEvidence`] payload struct and the [`EstimatesProvider`]
//! trait seam.  In Stage 1 these are contract-only types; no concrete provider
//! implementation is wired.  Live estimates fetching is deferred to Milestone 7.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// Analyst consensus-estimates evidence for a single ticker.
///
/// Stage 1: fields are defined for the full contract; live data population
/// is deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusEvidence {
    /// Ticker symbol (canonical uppercase).
    pub symbol: String,
    /// Consensus EPS estimate for the next reported quarter.
    pub eps_estimate: Option<f64>,
    /// Consensus revenue estimate (USD millions) for the next reported quarter.
    pub revenue_estimate_m: Option<f64>,
    /// Number of analysts contributing to this consensus.
    pub analyst_count: Option<u32>,
    /// ISO-8601 date of the estimate snapshot (`"YYYY-MM-DD"`).
    pub as_of_date: String,
}

/// Contract for any provider that can supply [`ConsensusEvidence`].
///
/// Stage 1 seam only.  Implementations are introduced in Milestone 7.
#[async_trait]
pub trait EstimatesProvider: Send + Sync {
    /// Fetch the most recent consensus estimates for `symbol` as of `as_of_date`
    /// (`"YYYY-MM-DD"`).
    ///
    /// Returns `Ok(None)` when no estimates are available.
    async fn fetch_consensus(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Option<ConsensusEvidence>, TradingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_evidence_serializes_and_deserializes() {
        let evidence = ConsensusEvidence {
            symbol: "MSFT".to_owned(),
            eps_estimate: Some(3.10),
            revenue_estimate_m: Some(65_500.0),
            analyst_count: Some(32),
            as_of_date: "2025-03-01".to_owned(),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: ConsensusEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn consensus_evidence_all_optional_fields_none_roundtrips() {
        let evidence = ConsensusEvidence {
            symbol: "TSLA".to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2025-04-01".to_owned(),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: ConsensusEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }
}
