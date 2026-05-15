//! Transcript evidence contract and fetch-outcome types.
//!
//! [`TranscriptEvidence`] carries structured per-segment data from earnings
//! call transcripts. [`TranscriptFetch`] wraps the four possible outcomes
//! of a transcript fetch attempt: found, not published, throttled, or
//! unavailable.

use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// A single speaker segment within an earnings-call transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Speaker name (e.g., "Tim Cook").
    pub speaker: String,
    /// Speaker title (e.g., "Chief Executive Officer").
    pub title: String,
    /// Spoken content for this segment.
    pub content: String,
    /// Provider-computed sentiment score for this segment, if available (`-1.0` to `1.0`).
    pub sentiment: Option<f64>,
}

/// Structured earnings-call transcript evidence.
///
/// `call_date` uses `"YYYYQN"` format (e.g., `"2025Q1"`) matching Alpha
/// Vantage's native quarter granularity. The canonical content is
/// `segments`; call [`rendered_content`](Self::rendered_content) for a
/// flat string when needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEvidence {
    /// Ticker symbol (canonical uppercase).
    pub symbol: String,
    /// Quarter identifier in `"YYYYQN"` format (e.g., `"2025Q1"`).
    pub call_date: String,
    /// Per-speaker transcript segments.
    pub segments: Vec<TranscriptSegment>,
}

impl TranscriptEvidence {
    /// Render all segments into a single string.
    ///
    /// Each segment is formatted as `"{speaker} ({title}): {content}"` and
    /// joined by `"\n\n"`. Returns an empty string when `segments` is empty.
    pub fn rendered_content(&self) -> String {
        self.segments
            .iter()
            .map(|s| format!("{} ({}): {}", s.speaker, s.title, s.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Outcome of a transcript-fetch attempt.
///
/// Each variant produces distinct prompt-layer language and audit-trail
/// metadata. Network/HTTP errors that persist after retries map to
/// `Err(TradingError)`, not to these variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TranscriptFetch {
    /// Transcript found and parsed.
    Found(TranscriptEvidence),
    /// API responded normally; no transcript published for this symbol/quarter yet.
    NotPublished,
    /// Every configured key returned a rate-limit signal within this call.
    Throttled,
    /// Recoverable transient failure (HTTP 5xx / timeout) persisted after retries.
    Unavailable,
}

/// Contract for any provider that can supply earnings-call transcripts.
///
/// Implementations return a [`TranscriptFetch`] enum rather than
/// `Option<TranscriptEvidence>` so callers can distinguish "not published"
/// from "throttled" from "unavailable".
#[async_trait::async_trait]
pub trait TranscriptProvider: Send + Sync {
    /// Fetch the transcript for `symbol` in the quarter identified by
    /// `as_of_date` (format `"YYYYQN"`, e.g., `"2025Q1"`).
    async fn fetch_transcript(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<TranscriptFetch, TradingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_evidence_with_segments_roundtrips() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "Chief Executive Officer".to_owned(),
                    content: "We had a great quarter...".to_owned(),
                    sentiment: Some(0.85),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "Chief Financial Officer".to_owned(),
                    content: "Revenue grew 5%...".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let json = serde_json::to_string(&evidence).expect("serialization");
        let recovered: TranscriptEvidence = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(evidence, recovered);
    }

    #[test]
    fn rendered_content_joins_segments() {
        let evidence = TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Tim Cook".to_owned(),
                    title: "CEO".to_owned(),
                    content: "Hello everyone.".to_owned(),
                    sentiment: Some(0.5),
                },
                TranscriptSegment {
                    speaker: "Luca Maestri".to_owned(),
                    title: "CFO".to_owned(),
                    content: "Thanks Tim.".to_owned(),
                    sentiment: None,
                },
            ],
        };
        let rendered = evidence.rendered_content();
        assert!(rendered.contains("Tim Cook (CEO): Hello everyone."));
        assert!(rendered.contains("Luca Maestri (CFO): Thanks Tim."));
    }

    #[test]
    fn rendered_content_empty_segments() {
        let evidence = TranscriptEvidence {
            symbol: "COIN".to_owned(),
            call_date: "2024Q4".to_owned(),
            segments: vec![],
        };
        assert_eq!(evidence.rendered_content(), "");
    }

    #[test]
    fn transcript_fetch_found_serializes() {
        let fetch = TranscriptFetch::Found(TranscriptEvidence {
            symbol: "AAPL".to_owned(),
            call_date: "2025Q1".to_owned(),
            segments: vec![],
        });
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert!(json.contains("Found"));
    }

    #[test]
    fn transcript_fetch_not_published_serializes() {
        let fetch: TranscriptFetch = TranscriptFetch::NotPublished;
        let json = serde_json::to_string(&fetch).expect("serialization");
        assert_eq!(json, "\"NotPublished\"");
    }

    #[test]
    fn call_date_is_quarter_format() {
        let evidence = TranscriptEvidence {
            symbol: "MSFT".to_owned(),
            call_date: "2025Q3".to_owned(),
            segments: vec![],
        };
        assert!(evidence.call_date.contains('Q'));
        assert_eq!(evidence.call_date, "2025Q3");
    }
}
