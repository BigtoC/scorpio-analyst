#![cfg(feature = "test-helpers")]

#[path = "workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use std::sync::Arc;

use scorpio_core::{state::TradingState, workflow::SnapshotStore};

pub async fn run_stubbed_pipeline(
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> (TradingState, Arc<SnapshotStore>, tempfile::TempDir) {
    let (pipeline, verify_store, dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "e2e-test.db",
        "e2e-test",
        max_debate_rounds,
        max_risk_rounds,
    )
    .await;

    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must complete successfully with stubs");

    (final_state, verify_store, dir)
}
