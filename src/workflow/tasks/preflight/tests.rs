use std::sync::Arc;

use graph_flow::{Context, Task};

use crate::{
    config::DataEnrichmentConfig,
    state::TradingState,
    workflow::{SnapshotStore, context_bridge::serialize_state_to_context},
};

use super::PreflightTask;

mod context_contract;
mod thesis;

/// Open a temporary on-disk snapshot store for preflight tests.
async fn test_store() -> (Arc<SnapshotStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("preflight-test.db");
    let store = SnapshotStore::new(Some(&path))
        .await
        .expect("store should open");
    (Arc::new(store), dir)
}

async fn run_preflight(
    symbol: &str,
    enrichment: DataEnrichmentConfig,
) -> graph_flow::Result<Context> {
    let (store, _dir) = test_store().await;
    run_preflight_with_store(symbol, enrichment, store).await
}

async fn run_preflight_with_store(
    symbol: &str,
    enrichment: DataEnrichmentConfig,
    store: Arc<SnapshotStore>,
) -> graph_flow::Result<Context> {
    let state = TradingState::new(symbol, "2026-01-15");
    let ctx = Context::new();
    serialize_state_to_context(&state, &ctx)
        .await
        .expect("state serialization");

    let task = PreflightTask::new(enrichment, store);
    task.run(ctx.clone()).await?;
    Ok(ctx)
}

#[tokio::test]
async fn preflight_fails_closed_on_invalid_symbol() {
    let state = TradingState::new("DROP;TABLE", "2026-01-15");
    let (store, _dir) = test_store().await;
    let ctx = Context::new();
    serialize_state_to_context(&state, &ctx)
        .await
        .expect("state serialization");

    let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
    let result = task.run(ctx).await;
    assert!(
        result.is_err(),
        "invalid symbol must cause preflight to fail"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("PreflightTask"),
        "error must identify the task: {msg}"
    );
}

#[tokio::test]
async fn preflight_fails_closed_on_missing_trading_state() {
    let (store, _dir) = test_store().await;
    let ctx = Context::new();
    let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
    let result = task.run(ctx).await;
    assert!(
        result.is_err(),
        "missing trading state must cause preflight to fail"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("PreflightTask"),
        "error must identify the task: {msg}"
    );
}
