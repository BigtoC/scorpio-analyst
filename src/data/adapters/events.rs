//! Stage 1 event-news evidence contract.
//!
//! Declares the [`EventNewsEvidence`] payload struct and the [`EventNewsProvider`]
//! trait seam.  In Stage 1 these are contract-only types; no concrete provider
//! implementation is wired.  Live event-news fetching is deferred to Milestone 7.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// An event-driven news evidence record for a single ticker.
///
/// Stage 1: fields are defined for the full contract; live data population
/// is deferred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventNewsEvidence {
    /// Ticker symbol (canonical uppercase).
    pub symbol: String,
    /// ISO-8601 timestamp of the event (`"YYYY-MM-DDTHH:MM:SSZ"`).
    pub event_timestamp: String,
    /// Short human-readable event category (e.g. `"earnings_release"`, `"merger_announcement"`).
    pub event_type: String,
    /// Full headline or summary text.
    pub headline: String,
    /// Estimated market-impact direction: `"positive"`, `"negative"`, or `"neutral"`.
    pub impact: Option<String>,
}

/// Contract for any provider that can supply [`EventNewsEvidence`].
///
/// Stage 1 seam only.  Implementations are introduced in Milestone 7.
#[async_trait]
pub trait EventNewsProvider: Send + Sync {
    /// Fetch event-driven news items for `symbol` within the window ending at
    /// `as_of_date` (`"YYYY-MM-DD"`).
    ///
    /// Returns an empty `Vec` when no events are available in the window.
    async fn fetch_event_news(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Vec<EventNewsEvidence>, TradingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_news_evidence_serializes_and_deserializes() {
        let evidence = EventNewsEvidence {
            symbol: "GOOGL".to_owned(),
            event_timestamp: "2025-01-28T21:00:00Z".to_owned(),
            event_type: "earnings_release".to_owned(),
            headline: "Alphabet beats Q4 expectations".to_owned(),
            impact: Some("positive".to_owned()),
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: EventNewsEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn event_news_evidence_without_impact_roundtrips() {
        let evidence = EventNewsEvidence {
            symbol: "META".to_owned(),
            event_timestamp: "2025-02-01T18:00:00Z".to_owned(),
            event_type: "product_launch".to_owned(),
            headline: "Meta announces new AR glasses".to_owned(),
            impact: None,
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: EventNewsEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }
}
