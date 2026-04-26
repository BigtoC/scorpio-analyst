#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_e2e_support.rs"]
mod workflow_pipeline_e2e_support;

use std::sync::Arc;

use chrono::Utc;
use graph_flow::{Context, NextAction, TaskResult};
use scorpio_core::{
    analysis_packs::{PackId, resolve_pack},
    error::TradingError,
    prompts::PromptBundle,
    state::{
        AssetShape, DataCoverageReport, Decision, DerivedValuation, EvidenceKind, EvidenceRecord,
        EvidenceSource, ExecutionStatus, FundamentalData, ProvenanceSummary, ScenarioValuation,
        TradingState,
    },
};
use workflow_pipeline_e2e_support::{
    make_pipeline, make_pipeline_from_pack, phase_from_number, run_stubbed_pipeline,
};

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
    assert!(final_state.fundamental_metrics().is_some());
    assert!(final_state.technical_indicators().is_some());
    assert!(final_state.market_sentiment().is_some());
    assert!(final_state.macro_news().is_some());
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
    assert!(!phase_names.contains(&"Researcher Debate Moderation"));
    assert!(!phase_names.contains(&"Risk Discussion Moderation"));
    assert!(final_state.debate_history.is_empty());
    assert!(final_state.risk_discussion_history.is_empty());
    assert!(final_state.final_execution_status.is_some());
}

#[tokio::test]
async fn from_pack_preflight_validates_the_supplied_manifest_not_the_registry_copy() {
    let mut pack = resolve_pack(PackId::Baseline);
    pack.name = "Custom baseline-shaped test pack".to_owned();
    pack.prompt_bundle = PromptBundle {
        trader: "".into(),
        ..pack.prompt_bundle
    };

    let (pipeline, _store, _dir) = make_pipeline_from_pack(
        &pack,
        "from-pack-custom-manifest.db",
        "from-pack-custom-manifest",
        "baseline",
        0,
        0,
    )
    .await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let err = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect_err("preflight should validate the supplied custom manifest");

    match err {
        TradingError::GraphFlow { task, cause, .. } => {
            assert_eq!(task, "preflight");
            assert!(
                cause.contains("trader"),
                "cause should mention missing trader slot: {cause}"
            );
        }
        other => panic!("expected preflight graph-flow error, got: {other:?}"),
    }
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
    assert!(final_1.current_thesis.is_some());
    assert!(final_2.current_thesis.is_some());
    assert!(final_2.prior_thesis.is_some());
    assert_eq!(
        final_2
            .prior_thesis
            .as_ref()
            .map(|thesis| thesis.symbol.as_str()),
        Some("AAPL")
    );
    assert_eq!(
        final_2
            .prior_thesis
            .as_ref()
            .map(|thesis| thesis.action.as_str()),
        Some("Buy")
    );

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
async fn from_pack_ignores_invalid_config_analysis_pack_and_runs_with_provided_pack() {
    let pack = resolve_pack(PackId::Baseline);
    let (pipeline, _store, _dir) = make_pipeline_from_pack(
        &pack,
        "from-pack-invalid-config.db",
        "from-pack-invalid-config",
        "not_a_real_pack",
        1,
        1,
    )
    .await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let final_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect("from_pack should honor the provided baseline pack even when config is invalid");

    assert_eq!(final_state.analysis_pack_name.as_deref(), Some("baseline"));
    assert_eq!(
        final_state
            .analysis_runtime_policy
            .as_ref()
            .map(|policy| policy.pack_id),
        Some(PackId::Baseline)
    );
}

#[tokio::test]
async fn from_pack_construction_accepts_non_selectable_pack_but_preflight_rejects_stub_bundle() {
    // The crypto pack is registered (resolvable via `resolve_pack`) but ships
    // `PromptBundle::empty()`. Construction via `from_pack` must succeed —
    // proving the API surface accepts any registered manifest — but
    // `PreflightTask`'s active-pack completeness gate must reject the run
    // *before* any analyst or model task fires, because an empty bundle
    // cannot produce real prompts. This test pins the contract that
    // construction-time acceptance is decoupled from runtime completeness.
    let pack = resolve_pack(PackId::CryptoDigitalAsset);
    let (pipeline, _store, _dir) = make_pipeline_from_pack(
        &pack,
        "from-pack-crypto.db",
        "from-pack-crypto",
        "baseline",
        0,
        0,
    )
    .await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed even for non-selectable packs");

    let err = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect_err(
            "preflight must reject the inactive crypto stub — its empty bundle cannot render \
             any prompts at runtime",
        );
    let msg = format!("{err}");
    assert!(
        msg.contains("PreflightTask"),
        "error must originate from PreflightTask, got: {msg}"
    );
    assert!(
        msg.contains("CryptoDigitalAsset"),
        "error must name the offending pack, got: {msg}"
    );
    assert!(
        msg.contains("incomplete") || msg.contains("missing"),
        "error must surface the completeness failure mode, got: {msg}"
    );
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

    assert!(snaps[0].fundamental_metrics().is_some());
    assert!(snaps[0].debate_history.is_empty());
    assert!(snaps[0].consensus_summary.is_none());
    assert!(snaps[0].trader_proposal.is_none());
    assert!(snaps[0].final_execution_status.is_none());

    assert!(snaps[1].fundamental_metrics().is_some());
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

    assert!(snaps[4].fundamental_metrics().is_some());
    assert!(snaps[4].consensus_summary.is_some());
    assert!(snaps[4].trader_proposal.is_some());
    assert!(snaps[4].aggressive_risk_report.is_some());
    assert!(snaps[4].final_execution_status.is_some());
    assert!(snaps[4].current_thesis.is_some());
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
        .push(scorpio_core::state::DebateMessage {
            role: "stale".to_owned(),
            content: "stale debate".to_owned(),
        });
    initial_state
        .risk_discussion_history
        .push(scorpio_core::state::DebateMessage {
            role: "stale".to_owned(),
            content: "stale risk".to_owned(),
        });
    initial_state.consensus_summary = Some("stale consensus".to_owned());
    initial_state.set_evidence_fundamental(EvidenceRecord {
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
        action: scorpio_core::state::TradeAction::Hold,
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
            .evidence_fundamental()
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
    assert!(final_state.prior_thesis.is_none());
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

/// Verify that `derived_valuation` injected into initial state is cleared by
/// `reset_cycle_outputs` before the pipeline re-derives it for the current run.
/// This prevents stale valuation from a prior symbol or run from leaking into
/// the trader and fund-manager prompts.
#[tokio::test]
async fn run_analysis_cycle_clears_stale_derived_valuation_from_reused_state() {
    let (pipeline, _store, _dir) =
        make_pipeline("stale-valuation.db", "stale-valuation", 1, 1).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    // Inject a stale derived_valuation that should NOT survive into the next cycle.
    let mut initial_state = TradingState::new("AAPL", "2026-03-20");
    initial_state.set_derived_valuation(DerivedValuation {
        asset_shape: AssetShape::Fund,
        scenario: ScenarioValuation::NotAssessed {
            reason: "stale_reason_from_prior_run".to_owned(),
        },
    });
    initial_state.trader_proposal = Some(scorpio_core::state::TradeProposal {
        action: scorpio_core::state::TradeAction::Hold,
        target_price: 1.0,
        stop_loss: 1.0,
        confidence: 0.1,
        rationale: "stale proposal".to_owned(),
        valuation_assessment: Some("stale valuation narrative".to_owned()),
        scenario_valuation: Some(ScenarioValuation::NotAssessed {
            reason: "stale_reason_from_prior_run".to_owned(),
        }),
    });

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must succeed with stale derived_valuation in initial state");

    // The stale NotAssessed valuation must not survive into the final state.
    // The stub AnalystTask does not inject derived_valuation, so it ends as None —
    // which is the correct cycle-safe outcome (reset happened).
    if let Some(dv) = final_state.derived_valuation() {
        // If the runtime did re-derive a value, it must NOT be the stale one.
        assert_ne!(
            dv.asset_shape,
            AssetShape::Fund,
            "stale Fund shape from prior run must not persist after reset"
        );
        if let ScenarioValuation::NotAssessed { reason } = &dv.scenario {
            assert_ne!(
                reason, "stale_reason_from_prior_run",
                "stale NotAssessed reason must not persist after reset"
            );
        }
    }
    let final_proposal = final_state
        .trader_proposal
        .as_ref()
        .expect("final trader proposal must be set");
    assert_ne!(
        final_proposal.valuation_assessment.as_deref(),
        Some("stale valuation narrative")
    );
    assert_ne!(
        final_proposal.scenario_valuation,
        Some(ScenarioValuation::NotAssessed {
            reason: "stale_reason_from_prior_run".to_owned(),
        })
    );
    // Whether the stub re-populates derived_valuation or not, the pipeline must complete.
    assert!(final_state.final_execution_status.is_some());
}
