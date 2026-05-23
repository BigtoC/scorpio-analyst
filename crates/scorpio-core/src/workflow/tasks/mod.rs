//! Graph-flow [`Task`] wrappers for all six pipeline phases.
//!
//! This facade preserves the workflow task API while splitting the
//! implementation into smaller, responsibility-focused modules.

mod accounting;
mod analyst;
mod auditor;
mod common;
pub(in crate::workflow) mod handoff;
pub mod preflight;
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
pub use auditor::AuditorTask;
#[allow(unused_imports)]
pub use common::{
    KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_SENTIMENT_NEWS, KEY_CACHED_VETTED_NEWS,
    KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_PROVIDER_CAPABILITIES,
    KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT, KEY_RISK_ROUND,
    KEY_ROUTING_FALLBACK_REASON, KEY_ROUTING_FLAGS, KEY_RUNTIME_PACK_ROUTE, KEY_RUNTIME_POLICY,
    KEY_TRANSCRIPT_FETCH_STATUS,
};
pub use preflight::PreflightTask;
pub use research::{BearishResearcherTask, BullishResearcherTask, DebateModeratorTask};
pub use risk::{AggressiveRiskTask, ConservativeRiskTask, NeutralRiskTask, RiskModeratorTask};
pub use trading::{FundManagerTask, TraderTask};
