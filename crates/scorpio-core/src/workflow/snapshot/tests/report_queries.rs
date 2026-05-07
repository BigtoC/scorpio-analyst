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
