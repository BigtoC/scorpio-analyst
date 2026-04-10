//! Event-news evidence contract and concrete Finnhub provider.
//!
//! Declares the [`EventNewsEvidence`] payload struct, the [`EventNewsProvider`]
//! trait seam, and the concrete [`FinnhubEventNewsProvider`] that normalizes
//! Finnhub company news into the adapter contract.

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

// ─── Concrete provider: Finnhub ─────────────────────────────────────────────

use chrono::NaiveDate;
use crate::constants::NEWS_ANALYSIS_DAYS;

/// Normalizes Finnhub [`CompanyNews`](finnhub::models::news::CompanyNews)
/// records into [`EventNewsEvidence`] payloads, filtering by `target_date`.
///
/// This provider:
/// - Fetches news for the 30 days preceding `as_of_date`.
/// - Excludes articles published after `as_of_date` (time-authority safety).
/// - Sanitizes headline/summary text for prompt-injection defense.
/// - Classifies event type and impact direction via keyword heuristics.
pub struct FinnhubEventNewsProvider {
    client: crate::data::FinnhubClient,
}

impl FinnhubEventNewsProvider {
    /// Construct a new provider backed by the given Finnhub client.
    pub fn new(client: crate::data::FinnhubClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl EventNewsProvider for FinnhubEventNewsProvider {
    async fn fetch_event_news(
        &self,
        symbol: &str,
        as_of_date: &str,
    ) -> Result<Vec<EventNewsEvidence>, TradingError> {
        let target = parse_date(as_of_date)?;
        let from = target - NEWS_ANALYSIS_DAYS;

        let from_str = from.format("%Y-%m-%d").to_string();
        let to_str = target.format("%Y-%m-%d").to_string();

        let raw = self
            .client
            .fetch_company_news(symbol, &from_str, &to_str)
            .await?;

        let target_end_of_day = target
            .and_hms_opt(23, 59, 59)
            .expect("valid HMS")
            .and_utc()
            .timestamp();

        let evidence: Vec<EventNewsEvidence> = raw
            .into_iter()
            .filter(|n| n.datetime <= target_end_of_day)
            .map(|n| normalize_company_news(symbol, n))
            .collect();

        Ok(evidence)
    }
}

/// Parse a `"YYYY-MM-DD"` date string into a `NaiveDate`.
fn parse_date(date_str: &str) -> Result<NaiveDate, TradingError> {
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| TradingError::AnalystError {
        agent: "enrichment".to_owned(),
        message: format!("invalid date '{date_str}': {e}"),
    })
}

/// Derive a coarse event-type label from headline + category keywords.
fn classify_event_type(headline: &str, category: &str) -> String {
    let text = format!("{headline} {category}").to_lowercase();
    if text.contains("earnings") || text.contains("quarterly results") || text.contains("eps") {
        "earnings_release".to_owned()
    } else if text.contains("merger") || text.contains("acquisition") || text.contains("acquire") {
        "merger_announcement".to_owned()
    } else if text.contains("dividend") {
        "dividend_announcement".to_owned()
    } else if text.contains("fda") || text.contains("approval") {
        "regulatory_event".to_owned()
    } else if text.contains("guidance") || text.contains("outlook") || text.contains("forecast") {
        "guidance_update".to_owned()
    } else {
        "company_news".to_owned()
    }
}

/// Derive impact direction from headline keywords.
fn classify_impact(headline: &str) -> Option<String> {
    let text = headline.to_lowercase();
    if text.contains("beat")
        || text.contains("surge")
        || text.contains("record")
        || text.contains("raise")
        || text.contains("upgrade")
    {
        Some("positive".to_owned())
    } else if text.contains("miss")
        || text.contains("plunge")
        || text.contains("cut")
        || text.contains("downgrade")
        || text.contains("warning")
    {
        Some("negative".to_owned())
    } else {
        None
    }
}

/// Normalize a single Finnhub `CompanyNews` into an `EventNewsEvidence`.
fn normalize_company_news(
    symbol: &str,
    news: finnhub::models::news::CompanyNews,
) -> EventNewsEvidence {
    let event_type = classify_event_type(&news.headline, &news.category);
    let impact = classify_impact(&news.headline);
    let ts = chrono::DateTime::from_timestamp(news.datetime, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned());

    EventNewsEvidence {
        symbol: symbol.to_ascii_uppercase(),
        event_timestamp: ts,
        event_type,
        headline: sanitize_text(&news.headline),
        impact,
    }
}

/// Lightweight text sanitization (strips HTML tags, collapses whitespace).
fn sanitize_text(text: &str) -> String {
    let mut buf = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                buf.push(' ');
            }
            _ if in_tag => {}
            _ => buf.push(ch),
        }
    }
    buf.split_whitespace().collect::<Vec<_>>().join(" ")
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

    // ── Normalization tests ──────────────────────────────────────────────

    #[test]
    fn normalize_company_news_produces_valid_evidence() {
        let news = finnhub::models::news::CompanyNews {
            category: "company".to_owned(),
            datetime: 1706475600, // 2024-01-28 21:00:00 UTC
            headline: "GOOGL beats Q4 earnings expectations".to_owned(),
            id: 12345,
            image: String::new(),
            related: "GOOGL".to_owned(),
            source: "Reuters".to_owned(),
            summary: "Revenue rose 10%".to_owned(),
            url: "https://example.com".to_owned(),
        };

        let evidence = normalize_company_news("GOOGL", news);
        assert_eq!(evidence.symbol, "GOOGL");
        assert_eq!(evidence.event_type, "earnings_release");
        assert_eq!(evidence.impact, Some("positive".to_owned()));
        assert!(evidence.event_timestamp.ends_with('Z'));
    }

    #[test]
    fn normalize_preserves_canonical_uppercase_symbol() {
        let news = finnhub::models::news::CompanyNews {
            category: "company".to_owned(),
            datetime: 1706475600,
            headline: "Some news headline".to_owned(),
            id: 1,
            image: String::new(),
            related: "aapl".to_owned(),
            source: "Test".to_owned(),
            summary: "summary".to_owned(),
            url: String::new(),
        };
        let evidence = normalize_company_news("aapl", news);
        assert_eq!(evidence.symbol, "AAPL");
    }

    // ── Classification tests ─────────────────────────────────────────────

    #[test]
    fn classify_event_type_identifies_earnings() {
        assert_eq!(
            classify_event_type("Q3 earnings beat estimates", "company"),
            "earnings_release"
        );
    }

    #[test]
    fn classify_event_type_identifies_merger() {
        assert_eq!(
            classify_event_type("Company to acquire rival", "merger"),
            "merger_announcement"
        );
    }

    #[test]
    fn classify_event_type_falls_back_to_company_news() {
        assert_eq!(
            classify_event_type("New product launched today", "company"),
            "company_news"
        );
    }

    #[test]
    fn classify_impact_positive() {
        assert_eq!(
            classify_impact("AAPL beats Q4 expectations"),
            Some("positive".to_owned())
        );
    }

    #[test]
    fn classify_impact_negative() {
        assert_eq!(
            classify_impact("Company issues profit warning"),
            Some("negative".to_owned())
        );
    }

    #[test]
    fn classify_impact_neutral() {
        assert_eq!(classify_impact("New store opening in Tokyo"), None);
    }

    // ── Time-authority tests ─────────────────────────────────────────────

    #[test]
    fn parse_date_valid() {
        let date = parse_date("2025-03-15").expect("valid date");
        assert_eq!(date, NaiveDate::from_ymd_opt(2025, 3, 15).unwrap());
    }

    #[test]
    fn parse_date_invalid_rejects() {
        assert!(parse_date("not-a-date").is_err());
    }

    // ── Sanitization tests ───────────────────────────────────────────────

    #[test]
    fn sanitize_text_strips_html_and_collapses_whitespace() {
        assert_eq!(sanitize_text("<b>Bold</b>   text   here"), "Bold text here");
    }
}
