use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Macroeconomic news and event data for the target asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewsData {
    pub articles: Vec<NewsArticle>,
    pub macro_events: Vec<MacroEvent>,
    pub summary: String,
}

/// A single news article or headline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewsArticle {
    pub title: String,
    pub source: String,
    pub published_at: String,
    pub relevance_score: Option<f64>,
    pub snippet: String,
}

/// Whether a macro event is expected to have a positive, negative, mixed, neutral,
/// or uncertain impact on the target asset.
///
/// `#[serde(rename_all = "snake_case")]` means the JSON representation is lowercase
/// (`"positive"`, `"negative"`, `"mixed"`, `"neutral"`, `"uncertain"`), matching the
/// strings previously used when this field was a free-form `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImpactDirection {
    Positive,
    Negative,
    Mixed,
    Neutral,
    Uncertain,
}

/// A macroeconomic event with a causal relationship to the asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MacroEvent {
    pub event: String,
    pub impact_direction: ImpactDirection,
    pub confidence: f64,
}
