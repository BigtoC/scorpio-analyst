use super::{in_memory_store, sample_state};
use crate::workflow::snapshot::{SnapshotPhase, THESIS_MEMORY_SCHEMA_VERSION};

#[tokio::test]
async fn list_executions_returns_correct_summaries_ordered_by_latest_activity() {
    let store = in_memory_store().await;

    let state1 = sample_state();
    let exec_id1 = state1.execution_id.to_string();
    store
        .save_snapshot(&exec_id1, SnapshotPhase::AnalystTeam, &state1, None)
        .await
        .expect("save first");

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let mut state2 = sample_state();
    state2.asset_symbol = "NVDA".to_string();
    let exec_id2 = state2.execution_id.to_string();
    store
        .save_snapshot(&exec_id2, SnapshotPhase::AnalystTeam, &state2, None)
        .await
        .expect("save second");

    let listing = store.list_executions().await.expect("list should succeed");

    assert_eq!(listing.summaries.len(), 2);
    assert_eq!(listing.stale_count, 0);
    assert_eq!(listing.summaries[0].symbol.as_deref(), Some("NVDA"));
    assert_eq!(listing.summaries[1].symbol.as_deref(), Some("AAPL"));
}

#[tokio::test]
async fn list_executions_on_empty_db_returns_empty_listing() {
    let store = in_memory_store().await;

    let listing = store.list_executions().await.expect("list should succeed");

    assert!(listing.summaries.is_empty());
    assert_eq!(listing.stale_count, 0);
}

#[tokio::test]
async fn list_executions_deduplicates_by_execution_id() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");
    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save phase 5");

    let listing = store.list_executions().await.expect("list should succeed");

    assert_eq!(
        listing.summaries.len(),
        1,
        "should deduplicate by execution_id"
    );
}

#[tokio::test]
async fn list_executions_excludes_rows_from_older_schema_versions_and_reports_stale_count() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id_current = state.execution_id.to_string();
    store
        .save_snapshot(&exec_id_current, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save current");

    let state_json = serde_json::to_string(&state).expect("serialize");
    for _ in 0..2 {
        let stale_exec_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json,
                 token_usage_json, created_at, symbol, schema_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&stale_exec_id)
        .bind(1i64)
        .bind("analyst_team")
        .bind(&state_json)
        .bind(None::<&str>)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind("AAPL")
        .bind(999i64)
        .execute(&store.pool)
        .await
        .expect("insert stale");
    }

    let listing = store.list_executions().await.expect("list");

    assert_eq!(
        listing.summaries.len(),
        1,
        "only current-schema rows are visible"
    );
    assert_eq!(listing.summaries[0].execution_id, exec_id_current);
    assert_eq!(
        listing.stale_count, 2,
        "stale executions counted but not surfaced"
    );
}

#[tokio::test]
async fn list_executions_parses_legacy_sqlite_datetime_format() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();
    let state_json = serde_json::to_string(&state).expect("serialize");

    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(1i64)
    .bind("analyst_team")
    .bind(&state_json)
    .bind(None::<&str>)
    .bind("2026-01-15 10:30:00")
    .bind("AAPL")
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .execute(&store.pool)
    .await
    .expect("insert legacy");

    let listing = store.list_executions().await.expect("list");

    assert_eq!(listing.summaries.len(), 1);
    assert_eq!(listing.summaries[0].execution_id, exec_id);
}

#[tokio::test]
async fn load_full_report_returns_all_phases_for_known_execution() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    for phase in [
        SnapshotPhase::AnalystTeam,
        SnapshotPhase::ResearcherDebate,
        SnapshotPhase::Trader,
        SnapshotPhase::RiskDiscussion,
        SnapshotPhase::FundManager,
    ] {
        store
            .save_snapshot(&exec_id, phase, &state, None)
            .await
            .expect("save");
    }

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 5);
    assert!(report.skipped_phases.is_empty());
    assert_eq!(report.snapshots.first().unwrap().phase_number, 1);
    assert_eq!(report.snapshots.last().unwrap().phase_number, 5);
}

#[tokio::test]
async fn load_full_report_with_unknown_id_returns_empty_report() {
    let store = in_memory_store().await;

    let report = store
        .load_full_report("non-existent-id")
        .await
        .expect("load");

    assert!(report.snapshots.is_empty());
    assert!(report.skipped_phases.is_empty());
}

#[tokio::test]
async fn load_full_report_returns_partial_phases_for_incomplete_run() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");
    store
        .save_snapshot(&exec_id, SnapshotPhase::ResearcherDebate, &state, None)
        .await
        .expect("save phase 2");

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(report.snapshots.len(), 2);
    assert!(report.skipped_phases.is_empty());
}

#[tokio::test]
async fn load_full_report_excludes_old_schema_rows() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    let state_json = serde_json::to_string(&state).expect("serialize");
    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(2i64)
    .bind("researcher_debate")
    .bind(&state_json)
    .bind(None::<&str>)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind("AAPL")
    .bind(999i64)
    .execute(&store.pool)
    .await
    .expect("insert stale");

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(
        report.snapshots.len(),
        1,
        "should only return current-schema phases"
    );
    assert_eq!(report.snapshots[0].phase_number, 1);
    assert!(
        report.skipped_phases.is_empty(),
        "stale rows are filtered, not skipped"
    );
}

#[tokio::test]
async fn load_full_report_with_only_old_schema_rows_returns_empty_report() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    let state_json = serde_json::to_string(&state).expect("serialize");
    for phase_num in 1..=5i64 {
        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json,
                 token_usage_json, created_at, symbol, schema_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&exec_id)
        .bind(phase_num)
        .bind("test_phase")
        .bind(&state_json)
        .bind(None::<&str>)
        .bind(chrono::Utc::now().to_rfc3339())
        .bind("AAPL")
        .bind(999i64)
        .execute(&store.pool)
        .await
        .expect("insert");
    }

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert!(
        report.snapshots.is_empty(),
        "all-stale execution must look not-found"
    );
}

#[tokio::test]
async fn load_full_report_tracks_phases_that_fail_deserialization() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("save phase 1");

    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json,
             token_usage_json, created_at, symbol, schema_version)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(2i64)
    .bind("researcher_debate")
    .bind("{invalid json")
    .bind(None::<&str>)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind("AAPL")
    .bind(THESIS_MEMORY_SCHEMA_VERSION)
    .execute(&store.pool)
    .await
    .expect("insert invalid json");

    let report = store.load_full_report(&exec_id).await.expect("load");

    assert_eq!(
        report.snapshots.len(),
        1,
        "should skip deserialization failure"
    );
    assert_eq!(
        report.skipped_phases,
        vec![2],
        "corrupt phase number tracked for CLI surface"
    );
}
