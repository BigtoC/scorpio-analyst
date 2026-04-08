#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_e2e_support.rs"]
mod workflow_pipeline_e2e_support;

use std::sync::Arc;

use chrono::Utc;
use graph_flow::{Context, NextAction, TaskResult};
use scorpio_analyst::{
    error::TradingError,
    state::{
        DataCoverageReport, Decision, EvidenceKind, EvidenceRecord, EvidenceSource,
        ExecutionStatus, FundamentalData, ProvenanceSummary, TradingState,
    },
};
use workflow_pipeline_e2e_support::{make_pipeline, phase_from_number, run_stubbed_pipeline};

#[tokio::test]
async fn run_analysis_cycle_success_path_populates_all_phases() {
    let (pipeline, verify_store, _dir) = make_pipeline("e2e-test.db", "e2e-test", 1, 1).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let caller_exec_id = initial_state.execution_id;

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must complete successfully with stubs");

    assert_ne!(final_state.execution_id, caller_exec_id);
    assert!(final_state.fundamental_metrics.is_some());
    assert!(final_state.technical_indicators.is_some());
    assert!(final_state.market_sentiment.is_some());
    assert!(final_state.macro_news.is_some());
    assert!(!final_state.debate_history.is_empty());
    assert!(final_state.consensus_summary.is_some());
    assert!(final_state.trader_proposal.is_some());
    assert!(final_state.aggressive_risk_report.is_some());
    assert!(final_state.conservative_risk_report.is_some());
    assert!(final_state.neutral_risk_report.is_some());
    assert!(!final_state.risk_discussion_history.is_empty());

    let exec_status = final_state
        .final_execution_status
        .as_ref()
        .expect("final execution status should be set");
    assert_eq!(exec_status.decision, Decision::Approved);
    assert!(final_state.current_thesis.is_some());
    assert_eq!(
        final_state
            .current_thesis
            .as_ref()
            .map(|thesis| thesis.decision.as_str()),
        Some("Approved")
    );

    let exec_id_str = final_state.execution_id.to_string();
    for phase_num in 1..=5 {
        let snapshot = verify_store
            .load_snapshot(&exec_id_str, phase_from_number(phase_num))
            .await
            .unwrap_or_else(|e| panic!("load_snapshot phase {phase_num} failed: {e}"));
        assert!(snapshot.is_some());
    }

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();
    assert!(phase_names.contains(&"Analyst Fan-Out"));
    assert!(phase_names.contains(&"Researcher Debate Round 1"));
    assert!(phase_names.contains(&"Researcher Debate Moderation"));
    assert!(phase_names.contains(&"Trader Synthesis"));
    assert!(phase_names.contains(&"Risk Discussion Round 1"));
    assert!(phase_names.contains(&"Risk Discussion Moderation"));
    assert!(phase_names.contains(&"Fund Manager Decision"));
}

#[tokio::test]
async fn e2e_zero_debate_zero_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(0, 0).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    let debate_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Researcher Debate Round"))
        .collect();
    let risk_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Risk Discussion Round"))
        .collect();
    assert!(debate_rounds.is_empty());
    assert!(risk_rounds.is_empty());
    assert!(phase_names.contains(&"Researcher Debate Moderation"));
    assert!(phase_names.contains(&"Risk Discussion Moderation"));
    assert!(final_state.debate_history.is_empty());
    assert_eq!(final_state.risk_discussion_history.len(), 1);
    assert!(final_state.final_execution_status.is_some());
}

#[tokio::test]
async fn e2e_multi_round_debate_and_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    for r in 1..=3 {
        let name = format!("Researcher Debate Round {r}");
        assert!(phase_names.contains(&name.as_str()));
    }
    assert!(!phase_names.contains(&"Researcher Debate Round 4"));

    for r in 1..=2 {
        let name = format!("Risk Discussion Round {r}");
        assert!(phase_names.contains(&name.as_str()));
    }
    assert!(!phase_names.contains(&"Risk Discussion Round 3"));

    assert_eq!(final_state.debate_history.len(), 6);
    assert_eq!(final_state.risk_discussion_history.len(), 8);
    assert!(final_state.final_execution_status.is_some());
}

#[tokio::test]
async fn e2e_zero_debate_multi_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(0, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    let debate_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Researcher Debate Round"))
        .collect();
    assert!(debate_rounds.is_empty());
    assert!(phase_names.contains(&"Risk Discussion Round 1"));
    assert!(phase_names.contains(&"Risk Discussion Round 2"));
    assert!(final_state.debate_history.is_empty());
    assert_eq!(final_state.risk_discussion_history.len(), 8);
    assert!(final_state.final_execution_status.is_some());
}

#[tokio::test]
async fn e2e_two_invocations_produce_distinct_execution_ids() {
    let (pipeline, verify_store, _dir) =
        make_pipeline("exec-id-test.db", "exec-id-test", 1, 1).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let final_1 = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect("run #1 must succeed");
    let final_2 = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect("run #2 must succeed");

    assert_ne!(final_1.execution_id, final_2.execution_id);

    let exec_id_1 = final_1.execution_id.to_string();
    let exec_id_2 = final_2.execution_id.to_string();
    for phase in 1..=5u8 {
        let snapshot_phase = phase_from_number(phase);
        let snap_1 = verify_store
            .load_snapshot(&exec_id_1, snapshot_phase)
            .await
            .unwrap_or_else(|e| panic!("load run#1 phase {phase}: {e}"));
        let snap_2 = verify_store
            .load_snapshot(&exec_id_2, snapshot_phase)
            .await
            .unwrap_or_else(|e| panic!("load run#2 phase {phase}: {e}"));
        assert!(snap_1.is_some());
        assert!(snap_2.is_some());
    }
}

#[tokio::test]
async fn e2e_snapshots_contain_boundary_appropriate_state() {
    let (final_state, verify_store, _dir) = run_stubbed_pipeline(1, 1).await;
    let exec_id = final_state.execution_id.to_string();

    let mut snaps = Vec::new();
    for phase in 1..=5u8 {
        let snapshot = verify_store
            .load_snapshot(&exec_id, phase_from_number(phase))
            .await
            .unwrap_or_else(|e| panic!("load phase {phase}: {e}"))
            .unwrap_or_else(|| panic!("phase {phase} snapshot must exist"));
        snaps.push(snapshot.state);
    }

    assert!(snaps[0].fundamental_metrics.is_some());
    assert!(snaps[0].debate_history.is_empty());
    assert!(snaps[0].consensus_summary.is_none());
    assert!(snaps[0].trader_proposal.is_none());
    assert!(snaps[0].final_execution_status.is_none());

    assert!(snaps[1].fundamental_metrics.is_some());
    assert!(!snaps[1].debate_history.is_empty());
    assert!(snaps[1].consensus_summary.is_some());
    assert!(snaps[1].trader_proposal.is_none());
    assert!(snaps[1].final_execution_status.is_none());

    assert!(snaps[2].trader_proposal.is_some());
    assert!(snaps[2].aggressive_risk_report.is_none());
    assert!(snaps[2].final_execution_status.is_none());

    assert!(snaps[3].trader_proposal.is_some());
    assert!(snaps[3].aggressive_risk_report.is_some());
    assert!(!snaps[3].risk_discussion_history.is_empty());
    assert!(snaps[3].final_execution_status.is_none());

    assert!(snaps[4].fundamental_metrics.is_some());
    assert!(snaps[4].consensus_summary.is_some());
    assert!(snaps[4].trader_proposal.is_some());
    assert!(snaps[4].aggressive_risk_report.is_some());
    assert!(snaps[4].final_execution_status.is_some());
}

#[tokio::test]
async fn step_ceiling_prevents_runaway_loop() {
    struct RunawayDebateModerator;

    #[async_trait::async_trait]
    impl graph_flow::Task for RunawayDebateModerator {
        fn id(&self) -> &str {
            "debate_moderator"
        }

        async fn run(&self, _context: Context) -> graph_flow::Result<TaskResult> {
            Ok(TaskResult::new(None, NextAction::Continue))
        }
    }

    let (pipeline, _store, _dir) = make_pipeline("ceiling-test.db", "ceiling-test", 3, 0).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");
    pipeline
        .replace_task_for_test(Arc::new(RunawayDebateModerator))
        .expect("runaway moderator replacement must succeed");

    let result = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await;
    assert!(result.is_err());

    match &result.unwrap_err() {
        TradingError::GraphFlow { phase, task, cause } => {
            assert_eq!(task, "step_ceiling");
            assert_eq!(phase, "pipeline_execution");
            assert!(cause.contains("exceeded maximum"));
            assert!(cause.contains("runaway loop"));
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

#[tokio::test]
async fn run_analysis_cycle_clears_stale_pipeline_outputs_from_reused_state() {
    let (pipeline, _store, _dir) = make_pipeline("reused-state.db", "reused-state", 1, 1).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let mut initial_state = TradingState::new("AAPL", "2026-03-20");
    initial_state
        .debate_history
        .push(scorpio_analyst::state::DebateMessage {
            role: "stale".to_owned(),
            content: "stale debate".to_owned(),
        });
    initial_state
        .risk_discussion_history
        .push(scorpio_analyst::state::DebateMessage {
            role: "stale".to_owned(),
            content: "stale risk".to_owned(),
        });
    initial_state.consensus_summary = Some("stale consensus".to_owned());
    initial_state.evidence_fundamental = Some(EvidenceRecord {
        kind: EvidenceKind::Fundamental,
        payload: FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(999.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "stale fundamentals".to_owned(),
        },
        sources: vec![EvidenceSource {
            provider: "stale-provider".to_owned(),
            datasets: vec!["stale-dataset".to_owned()],
            fetched_at: Utc::now(),
            effective_at: None,
            url: None,
            citation: None,
        }],
        quality_flags: vec![],
    });
    initial_state.data_coverage = Some(DataCoverageReport {
        required_inputs: vec!["stale".to_owned()],
        missing_inputs: vec!["stale-missing".to_owned()],
    });
    initial_state.provenance_summary = Some(ProvenanceSummary {
        providers_used: vec!["stale-provider".to_owned()],
    });
    initial_state.final_execution_status = Some(ExecutionStatus {
        decision: Decision::Rejected,
        action: scorpio_analyst::state::TradeAction::Hold,
        rationale: "stale".to_owned(),
        decided_at: "2026-01-01T00:00:00Z".to_owned(),
        entry_guidance: None,
        suggested_position: None,
    });

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must succeed with reused state");

    assert_ne!(
        final_state.consensus_summary.as_deref(),
        Some("stale consensus")
    );
    assert_ne!(
        final_state
            .evidence_fundamental
            .as_ref()
            .and_then(|record| record.payload.pe_ratio),
        Some(999.0)
    );
    assert_eq!(
        final_state
            .data_coverage
            .as_ref()
            .expect("data coverage must be recomputed")
            .required_inputs,
        vec!["fundamentals", "sentiment", "news", "technical"]
    );
    assert!(
        !final_state
            .provenance_summary
            .as_ref()
            .expect("provenance summary must be recomputed")
            .providers_used
            .iter()
            .any(|provider| provider == "stale-provider")
    );
    assert!(final_state.final_execution_status.is_some());
    assert!(final_state.current_thesis.is_some());
    assert!(
        final_state
            .debate_history
            .iter()
            .all(|msg| msg.role != "stale")
    );
    assert!(
        final_state
            .risk_discussion_history
            .iter()
            .all(|msg| msg.role != "stale")
    );
}
