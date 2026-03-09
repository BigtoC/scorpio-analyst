use serde::{Deserialize, Serialize};

/// Aggregated social-media and market sentiment analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentimentData {
    pub overall_score: f64,
    pub source_breakdown: Vec<SentimentSource>,
    pub engagement_peaks: Vec<EngagementPeak>,
    pub summary: String,
}

/// Sentiment contribution from a single data source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SentimentSource {
    pub source_name: String,
    pub score: f64,
    pub sample_size: u64,
}

/// A notable peak in social-media engagement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngagementPeak {
    pub timestamp: String,
    pub platform: String,
    pub intensity: f64,
}
