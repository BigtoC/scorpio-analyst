#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use scorpio_core::{
    analysis_packs::{PackId, resolve_pack},
    state::{TradingState, auditor::AuditStatus},
    workflow::{SnapshotPhase, run_analysis_cycle},
};

use workflow_pipeline_make_pipeline::make_pipeline_from_pack;

#[tokio::test]
async fn auditor_failure_is_fail_open_and_preserves_deterministic_findings() {
    let mut pack = resolve_pack(PackId::Baseline);
    pack.auditor_enabled = true;

    let (pipeline, _verify_store, _dir) = make_pipeline_from_pack(
        &pack,
        "workflow-auditor-fail-open.db",
        "workflow-auditor-fail-open",
        "baseline",
        0,
        0,
    )
    .await;
    pipeline
        .install_stub_tasks_except_auditor_for_test()
        .expect("stub install must succeed");

    // StubTraderTask sets stop_loss (200.0) > target_price (195.0), if this which
    // triggers the BUY-stop_loss-above-target deterministic check without
    // depending on any network-fetched current_price.
    let initial_state = TradingState::new("AAPL", "2026-03-20");

    let final_state = run_analysis_cycle(&pipeline, initial_state)
        .await
        .expect("pipeline must complete fail-open");

    assert_eq!(final_state.audit_status, AuditStatus::FailedOpen);
    let report = final_state
        .audit_report
        .as_ref()
        .expect("deterministic report must survive fail-open");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.location == "trader_proposal.stop_loss")
    );
    assert_eq!(report.auditor_model_id, "runtime_unavailable");

    let exec_id = final_state.execution_id.to_string();
    assert!(
        _verify_store
            .load_snapshot(&exec_id, SnapshotPhase::FundManager)
            .await
            .expect("load phase 5")
            .is_some()
    );
}
