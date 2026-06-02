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

    state.set_evidence_fundamental(EvidenceRecord {
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
        loaded.state.evidence_fundamental().is_some(),
        "evidence_fundamental must survive snapshot"
    );
    assert_eq!(
        loaded
            .state
            .evidence_fundamental()
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
    let cfg_path = dir.path().join("config.toml");
    std::fs::write(
        &cfg_path,
        r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
    )
    .expect("config file should be written");
    let mut config = crate::config::Config::load_from(&cfg_path).expect("config load");
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

#[cfg(unix)]
#[tokio::test]
async fn snapshot_db_file_is_user_only_readable() {
    use std::os::unix::fs::PermissionsExt;

    // Holdings persisted via the account_positions feature land in this DB, so
    // the file must not be group/other readable (data-at-rest gate).
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("perms.db");
    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("store should open");
    drop(store);

    let mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "snapshot db must be user-only (rw-------), got {mode:o}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_store_leaves_preexisting_parent_dir_permissions_untouched() {
    use std::os::unix::fs::PermissionsExt;

    // The store must not narrow a pre-existing (possibly shared) operator-
    // configured directory as a side effect of opening — only the DB file.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let db_path = dir.path().join("preexisting.db");

    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("store should open");
    drop(store);

    let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        dir_mode, 0o755,
        "pre-existing parent dir must be left untouched, got {dir_mode:o}"
    );
    let file_mode = std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "DB file must still be user-only");
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_store_tightens_a_directory_it_creates() {
    use std::os::unix::fs::PermissionsExt;

    // A directory the store creates (e.g. the default data dir on first run) is
    // narrowed to user-only, which also covers any SQLite -wal/-shm sidecars.
    let dir = tempfile::tempdir().expect("temp dir");
    let created = dir.path().join("scorpio-data"); // does not exist yet
    let db_path = created.join("snap.db");

    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("store should open");
    drop(store);

    let dir_mode = std::fs::metadata(&created).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        dir_mode, 0o700,
        "a store-created directory must be user-only, got {dir_mode:o}"
    );
}

#[tokio::test]
async fn account_positions_survive_snapshot_round_trip() {
    use crate::state::{AccountPosition, AccountPositionsState, AccountSnapshot, PositionSide};

    let store = in_memory_store().await;
    let mut state = crate::state::TradingState::new("AAPL", "2026-01-15");
    let exec_id = state.execution_id.to_string();
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-abc123".to_owned()),
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(250_000.0),
        positions: vec![AccountPosition {
            code: "US.AAPL".to_owned(),
            name: "Apple".to_owned(),
            qty: 100.0,
            can_sell_qty: 100.0,
            cost_price: Some(150.0),
            current_price: Some(185.42),
            market_value: Some(18_542.0),
            pl_ratio: Some(0.236),
            pl_val: Some(3_542.0),
            currency: "USD".to_owned(),
            side: PositionSide::Long,
        }],
    });

    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save should succeed");
    let loaded = store
        .load_snapshot(&exec_id, SnapshotPhase::FundManager)
        .await
        .expect("load should succeed")
        .expect("snapshot should exist");

    assert_eq!(loaded.state.account_positions, state.account_positions);
}

#[test]
fn persisted_account_positions_contain_no_raw_account_id() {
    use crate::state::{AccountPositionsState, AccountSnapshot};

    // `save_snapshot` persists `serde_json::to_string(state)`, so asserting on
    // the serialized state tests the exact bytes that hit disk — no store needed.
    let mut state = crate::state::TradingState::new("AAPL", "2026-01-15");
    state.account_positions = AccountPositionsState::Available(AccountSnapshot {
        account_label: Some("acct-9f8e7d".to_owned()), // redacted hash, not the raw id
        market: "US".to_owned(),
        currency: "USD".to_owned(),
        total_market_value: Some(0.0),
        positions: vec![],
    });
    let raw = serde_json::to_string(&state).expect("state serializes");
    assert!(
        !raw.contains("\"acc_id\"") && !raw.contains("\"accID\""),
        "persisted snapshot must not contain a raw account id field: {raw}"
    );
}

#[test]
fn legacy_snapshot_without_account_positions_loads_as_disabled() {
    use crate::state::{AccountPositionsState, TradingState};
    // A serialized state from before this feature has no `account_positions` key.
    let mut value = serde_json::to_value(TradingState::new("AAPL", "2026-01-15")).unwrap();
    value.as_object_mut().unwrap().remove("account_positions");
    let legacy_json = serde_json::to_string(&value).unwrap();

    let state: TradingState =
        serde_json::from_str(&legacy_json).expect("legacy snapshot must still deserialize");
    assert_eq!(state.account_positions, AccountPositionsState::Disabled);
}
