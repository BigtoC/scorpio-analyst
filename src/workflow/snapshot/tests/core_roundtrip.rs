use chrono::Utc;

use super::{in_memory_store, sample_state};
use crate::state::{
    DataCoverageReport, EvidenceKind, EvidenceRecord, EvidenceSource, FundamentalData,
    ProvenanceSummary,
};
use crate::workflow::snapshot::{SnapshotPhase, SnapshotStore};

#[test]
fn snapshot_phase_reports_storage_number_and_name() {
    assert_eq!(SnapshotPhase::AnalystTeam.number(), 1);
    assert_eq!(SnapshotPhase::AnalystTeam.name(), "analyst_team");
    assert_eq!(SnapshotPhase::ResearcherDebate.number(), 2);
    assert_eq!(SnapshotPhase::ResearcherDebate.name(), "researcher_debate");
    assert_eq!(SnapshotPhase::Trader.number(), 3);
    assert_eq!(SnapshotPhase::Trader.name(), "trader");
    assert_eq!(SnapshotPhase::RiskDiscussion.number(), 4);
    assert_eq!(SnapshotPhase::RiskDiscussion.name(), "risk_discussion");
    assert_eq!(SnapshotPhase::FundManager.number(), 5);
    assert_eq!(SnapshotPhase::FundManager.name(), "fund_manager");
}

#[tokio::test]
async fn save_and_load_round_trip() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save should succeed");

    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect("load should succeed")
        .expect("snapshot should exist");

    assert_eq!(loaded.state.asset_symbol, state.asset_symbol);
    assert_eq!(loaded.state.target_date, state.target_date);
    assert!(loaded.token_usage.is_none());
}

#[tokio::test]
async fn upsert_replaces_existing_snapshot() {
    let store = in_memory_store().await;
    let mut state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .unwrap();

    state.target_date = "2026-03-19".to_string();
    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .unwrap();

    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .unwrap()
        .expect("snapshot should exist");

    assert_eq!(loaded.state.target_date, "2026-03-19");
}

#[tokio::test]
async fn missing_snapshot_returns_none() {
    let store = in_memory_store().await;

    let result = store
        .load_snapshot("non-existent-id", SnapshotPhase::FundManager)
        .await
        .expect("query should not fail");

    assert!(result.is_none());
}

#[tokio::test]
async fn save_with_token_usage_round_trip() {
    use crate::state::AgentTokenUsage;

    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    let usage = vec![AgentTokenUsage {
        agent_name: "FundamentalAnalyst".to_string(),
        model_id: "gpt-4o-mini".to_string(),
        token_counts_available: true,
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
        latency_ms: 1200,
        rate_limit_wait_ms: 0,
    }];

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, Some(&usage))
        .await
        .unwrap();

    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .unwrap()
        .expect("snapshot should exist");

    let loaded_usage = loaded.token_usage.expect("token usage should be present");
    assert_eq!(loaded_usage.len(), 1);
    assert_eq!(loaded_usage[0].agent_name, "FundamentalAnalyst");
    assert_eq!(loaded_usage[0].total_tokens, 150);
}

#[tokio::test]
async fn save_snapshot_uses_typed_phase_api() {
    let store = in_memory_store().await;
    let state = sample_state();

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::Trader,
            &state,
            None,
        )
        .await
        .expect("typed phase save should succeed");

    let loaded = store
        .load_snapshot(&state.execution_id.to_string(), SnapshotPhase::Trader)
        .await
        .expect("load should succeed")
        .expect("snapshot should exist");

    assert_eq!(loaded.state.asset_symbol, state.asset_symbol);
}

#[tokio::test]
async fn snapshot_store_implements_debug() {
    let store = in_memory_store().await;
    let rendered = format!("{store:?}");
    assert!(rendered.contains("SnapshotStore"));
}

#[tokio::test]
async fn evidence_fields_survive_snapshot_round_trip() {
    let store = in_memory_store().await;
    let mut state = crate::state::TradingState::new("TSLA", "2026-01-15");
    let exec_id = state.execution_id.to_string();

    state.evidence_fundamental = Some(EvidenceRecord {
        kind: EvidenceKind::Fundamental,
        payload: FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(42.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "snapshot test".to_owned(),
        },
        sources: vec![EvidenceSource {
            provider: "finnhub".to_owned(),
            datasets: vec!["fundamentals".to_owned()],
            fetched_at: Utc::now(),
            effective_at: None,
            url: None,
            citation: None,
        }],
        quality_flags: vec![],
    });
    state.data_coverage = Some(DataCoverageReport {
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        missing_inputs: vec!["technical".to_owned()],
    });
    state.provenance_summary = Some(ProvenanceSummary {
        providers_used: vec!["finnhub".to_owned()],
    });

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save should succeed");

    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect("load should succeed")
        .expect("snapshot should exist");

    assert!(
        loaded.state.evidence_fundamental.is_some(),
        "evidence_fundamental must survive snapshot"
    );
    assert_eq!(
        loaded
            .state
            .evidence_fundamental
            .as_ref()
            .unwrap()
            .payload
            .pe_ratio,
        Some(42.0)
    );
    assert_eq!(
        loaded.state.data_coverage.as_ref().unwrap().missing_inputs,
        vec!["technical"]
    );
    assert_eq!(
        loaded
            .state
            .provenance_summary
            .as_ref()
            .unwrap()
            .providers_used,
        vec!["finnhub"]
    );
}

#[tokio::test]
async fn from_config_uses_expanded_snapshot_db_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("configured.db");
    let mut config = crate::config::Config::load_from("config.toml").expect("config load");
    config.storage.snapshot_db_path = db_path.to_string_lossy().into_owned();

    let store = SnapshotStore::from_config(&config)
        .await
        .expect("store should open from config path");

    assert!(
        db_path.exists(),
        "configured snapshot db path should be created"
    );
    drop(store);
}
