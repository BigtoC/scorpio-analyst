//! Hermetic integration tests for [`scorpio_core::app::AnalysisRuntime`].
//!
//! The `from_pipeline` constructor is gated on the `test-helpers` feature so
//! we can wrap a stubbed-task pipeline (built via `workflow::test_support`)
//! without having to go through the real provider/model assembly path inside
//! `AnalysisRuntime::new`.

#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use scorpio_core::app::AnalysisRuntime;

#[tokio::test]
async fn run_returns_state_with_final_execution_status() {
    let (pipeline, _verify_store, _dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "app-runtime-success.db",
        "app-runtime",
        0,
        0,
    )
    .await;

    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let runtime = AnalysisRuntime::from_pipeline(pipeline);
    let state = runtime
        .run("AAPL")
        .await
        .expect("stubbed analysis cycle should succeed");

    assert!(
        state.final_execution_status.is_some(),
        "AnalysisRuntime::run must surface a final_execution_status"
    );
    assert_eq!(state.asset_symbol, "AAPL");
}

#[tokio::test]
async fn run_rejects_invalid_symbol_before_executing_pipeline() {
    let (pipeline, _verify_store, _dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "app-runtime-reject.db",
        "app-runtime",
        0,
        0,
    )
    .await;

    // Stub tasks intentionally NOT installed: the symbol guard must trip
    // before any pipeline task is scheduled.
    let runtime = AnalysisRuntime::from_pipeline(pipeline);
    let err = runtime
        .run("NOT A VALID SYMBOL!!")
        .await
        .expect_err("invalid symbol must fail validation");

    assert!(
        format!("{err:#}").contains("invalid symbol"),
        "symbol validation should surface the original schema-violation message; got {err:#}"
    );
}
