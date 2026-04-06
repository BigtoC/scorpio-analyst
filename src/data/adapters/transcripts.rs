//! Stage 1 transcript evidence contract.
//!
//! Declares the [`TranscriptEvidence`] payload struct and the [`TranscriptProvider`]
//! trait seam.  In Stage 1 these are contract-only types; no concrete provider
//! implementation is wired.  Live transcript fetching is deferred to Milestone 7.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// A single earnings-call or conference-call transcript evidence record.
///
/// Stage 1: fields are defined for the full contract; live data population
/// is deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEvidence {
    /// Ticker symbol this transcript is associated with (canonical uppercase).
    pub symbol: String,
    /// ISO-8601 date string of the earnings call (`"YYYY-MM-DD"`).
    pub call_date: String,
    /// Participant-attributed excerpts or full transcript text.
    pub content: String,
    /// Sentiment score derived from the transcript, if available (`-1.0` to `1.0`).
    pub sentiment_score: Option<f64>,
}

/// Contract for any provider that can supply [`TranscriptEvidence`].
///
/// Stage 1 seam only.  Implementations are introduced in Milestone 7.
#[async_trait]
pub trait TranscriptProvider: Send + Sync {
    /// Fetch the most recent available transcript for `symbol` on or before
    /// `as_of_date` (`"YYYY-MM-DD"`).
    ///
    /// Returns `Ok(None)` when no transcript is available within the evidence
    /// age window.
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Option<TranscriptEvidence>, TradingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_evidence_serializes_and_deserializes() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025-01-30".to_owned(),
            content: "CEO: we had a great quarter...".to_owned(),
            sentiment_score: Some(0.62),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: TranscriptEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn transcript_evidence_without_sentiment_roundtrips() {
        let evidence = TranscriptEvidence {
            symbol: "NVDA".to_owned(),
            call_date: "2025-02-15".to_owned(),
            content: "Strong AI demand continues...".to_owned(),
            sentiment_score: None,
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: TranscriptEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }
}
