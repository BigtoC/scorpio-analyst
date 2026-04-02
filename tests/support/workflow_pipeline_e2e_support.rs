#![cfg(feature = "test-helpers")]

#[path = "workflow_pipeline_stubbed_support.rs"]
mod workflow_pipeline_stubbed_support;

#[allow(clippy::duplicate_mod)]
#[path = "workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use std::sync::Arc;

use scorpio_analyst::{
    state::TradingState,
    workflow::{SnapshotPhase, SnapshotStore},
};

pub use workflow_pipeline_make_pipeline::make_pipeline;

pub fn phase_from_number(phase: u8) -> SnapshotPhase {
    match phase {
        1 => SnapshotPhase::AnalystTeam,
        2 => SnapshotPhase::ResearcherDebate,
        3 => SnapshotPhase::Trader,
        4 => SnapshotPhase::RiskDiscussion,
        5 => SnapshotPhase::FundManager,
        _ => panic!("unsupported snapshot phase: {phase}"),
    }
}

pub async fn run_stubbed_pipeline(
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> (TradingState, Arc<SnapshotStore>, tempfile::TempDir) {
    workflow_pipeline_stubbed_support::run_stubbed_pipeline(max_debate_rounds, max_risk_rounds)
        .await
}
