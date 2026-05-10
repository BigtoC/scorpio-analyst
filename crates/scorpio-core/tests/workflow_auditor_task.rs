#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use scorpio_core::{
    analysis_packs::{PackId, resolve_pack},
    state::{TradeAction, TradeProposal, TradingState, auditor::AuditStatus},
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

    let mut initial_state = TradingState::new("AAPL", "2026-03-20");
    initial_state.current_price = Some(120.0);
    initial_state.trader_proposal = Some(TradeProposal {
        action: TradeAction::Buy,
        target_price: 100.0,
        stop_loss: 95.0,
        confidence: 0.8,
        rationale: "test rationale".to_owned(),
        valuation_assessment: None,
        scenario_valuation: None,
    });

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
            .any(|finding| finding.location == "trader_proposal.target_price")
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
