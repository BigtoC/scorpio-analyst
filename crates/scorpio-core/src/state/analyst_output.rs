//! Canonical sum type for an analyst's structured output.
//!
//! Phase 6 establishes [`AnalystOutput`] as the typed payload carried by
//! future registry-driven analyst dispatch. Today the analyst tasks still
//! write payloads directly into [`crate::state::EquityState`] fields
//! because the dispatch lives in `workflow/tasks/analyst.rs`; the enum is
//! introduced now so Phase 7's builder work and future crypto analyst
//! implementations can standardize on a single shape without another
//! schema bump.
use serde::{Deserialize, Serialize};

use super::{FundamentalData, NewsData, SentimentData, TechnicalData};

/// Discriminated union of every analyst payload the pipeline understands.
///
/// `#[non_exhaustive]` so adding crypto payloads in a follow-up change is
/// not a breaking addition for external consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnalystOutput {
    Fundamental(FundamentalData),
    Sentiment(SentimentData),
    News(NewsData),
    Technical(Box<TechnicalData>),
    /// Placeholder — crypto tokenomics analyst output.
    Tokenomics(()),
    /// Placeholder — crypto on-chain analyst output.
    OnChain(()),
    /// Placeholder — crypto social analyst output.
    Social(()),
    /// Placeholder — crypto derivatives analyst output.
    Derivatives(()),
}
