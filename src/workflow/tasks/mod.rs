//! Graph-flow [`Task`] wrappers for all five pipeline phases.
//!
//! This facade preserves the workflow task API while splitting the
//! implementation into smaller, responsibility-focused modules.

mod accounting;
mod analyst;
mod common;
mod research;
mod risk;
mod runtime;
mod trading;

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

#[cfg(test)]
mod tests;

pub use analyst::{
    AnalystSyncTask, FundamentalAnalystTask, NewsAnalystTask, SentimentAnalystTask,
    TechnicalAnalystTask,
};
pub use common::{
    KEY_CACHED_NEWS, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND,
};
pub use research::{BearishResearcherTask, BullishResearcherTask, DebateModeratorTask};
pub use risk::{AggressiveRiskTask, ConservativeRiskTask, NeutralRiskTask, RiskModeratorTask};
pub use trading::{FundManagerTask, TraderTask};
