use serde::{Deserialize, Serialize};

/// Macro-economic news and event data for the target asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewsData {
    pub articles: Vec<NewsArticle>,
    pub macro_events: Vec<MacroEvent>,
    pub summary: String,
}

/// A single news article or headline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewsArticle {
    pub title: String,
    pub source: String,
    pub published_at: String,
    pub relevance_score: Option<f64>,
    pub snippet: String,
}

/// A macro-economic event with a causal relationship to the asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroEvent {
    pub event: String,
    pub impact_direction: String,
    pub confidence: f64,
}
