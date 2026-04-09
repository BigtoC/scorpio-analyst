use super::{in_memory_store, sample_thesis};
use crate::error::TradingError;
use crate::state::TradingState;
use crate::workflow::snapshot::SnapshotPhase;

#[tokio::test]
async fn load_prior_thesis_skips_snapshots_without_current_thesis() {
    let store = in_memory_store().await;

    let state = TradingState::new("AAPL", "2026-04-07");
    assert!(state.current_thesis.is_none());

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert!(
        result.is_none(),
        "phase-5 snapshot without current_thesis should yield None"
    );
}

#[tokio::test]
async fn load_prior_thesis_supports_legacy_rows_without_symbol_column_data() {
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-07");
    state.current_thesis = Some(sample_thesis());

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save should succeed");

    sqlx::query(
        "UPDATE phase_snapshots SET symbol = NULL WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("legacy-row update should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert_eq!(
        result.expect("legacy row should still be found").action,
        "Buy"
    );
}

#[tokio::test]
async fn load_prior_thesis_skips_rows_from_future_schema_versions() {
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-07");
    state.current_thesis = Some(sample_thesis());

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save should succeed");

    sqlx::query(
        "UPDATE phase_snapshots SET schema_version = 2 WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("schema-version update should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert!(
        result.is_none(),
        "unsupported future schema version must be skipped"
    );
}

#[tokio::test]
async fn load_prior_thesis_returns_storage_error_for_malformed_supported_payload() {
    let store = in_memory_store().await;
    let state = TradingState::new("AAPL", "2026-04-07");

    store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &state,
            None,
        )
        .await
        .expect("save should succeed");

    sqlx::query(
        "UPDATE phase_snapshots SET trading_state_json = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind("{malformed-json")
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("malformed update should succeed");

    let result = store.load_prior_thesis_for_symbol("AAPL", 30).await;

    assert!(matches!(result, Err(TradingError::Storage(_))));
}

#[tokio::test]
async fn save_snapshot_persists_symbol_column() {
    let store = in_memory_store().await;
    let state = TradingState::new("MSFT", "2026-04-07");
    let exec_id = state.execution_id.to_string();

    store
        .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
        .await
        .expect("save should succeed");

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM phase_snapshots WHERE symbol = ? AND phase_number = 5",
    )
    .bind("MSFT")
    .fetch_one(&store.pool)
    .await
    .expect("count query should succeed");

    assert_eq!(count.0, 1, "one phase-5 snapshot for MSFT should exist");
}
