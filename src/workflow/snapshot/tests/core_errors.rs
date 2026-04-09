use chrono::Utc;

use super::{FailingSerialize, in_memory_store, sample_state};
use crate::error::TradingError;
use crate::workflow::snapshot::{SnapshotPhase, serialize_snapshot_json};

#[test]
fn storage_error_preserves_source() {
    let error = TradingError::Storage(anyhow::anyhow!("snapshot failed"));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn serialize_snapshot_json_returns_storage_error_for_serialization_failures() {
    let error = serialize_snapshot_json(&FailingSerialize, "failing value")
        .expect_err("intentional serializer failure should propagate");

    assert!(matches!(error, TradingError::Storage(_)));
}

#[tokio::test]
async fn save_snapshot_returns_storage_error_for_runtime_failures() {
    let store = in_memory_store().await;
    let state = sample_state();

    store.close_for_test().await;

    let error = store
        .save_snapshot(
            &state.execution_id.to_string(),
            SnapshotPhase::AnalystTeam,
            &state,
            None,
        )
        .await
        .expect_err("closed pool should fail");

    assert!(matches!(error, TradingError::Storage(_)));
}

#[tokio::test]
async fn load_snapshot_returns_storage_error_for_runtime_failures() {
    let store = in_memory_store().await;

    store.close_for_test().await;

    let error = store
        .load_snapshot("exec-id", SnapshotPhase::AnalystTeam)
        .await
        .expect_err("closed pool should fail");

    assert!(matches!(error, TradingError::Storage(_)));
}

#[tokio::test]
async fn load_snapshot_returns_storage_error_for_decode_failures() {
    let store = in_memory_store().await;
    let state = sample_state();
    let exec_id = state.execution_id.to_string();

    sqlx::query(
        "INSERT INTO phase_snapshots
            (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&exec_id)
    .bind(SnapshotPhase::AnalystTeam.number() as i64)
    .bind(SnapshotPhase::AnalystTeam.name())
    .bind("{\"asset_symbol\":true}")
    .bind(Option::<&str>::None)
    .bind(Utc::now().to_rfc3339())
    .execute(&store.pool)
    .await
    .expect("seed invalid row");

    let error = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect_err("invalid snapshot JSON should fail decode");

    assert!(matches!(error, TradingError::Storage(_)));
}
