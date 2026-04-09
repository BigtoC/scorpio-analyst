use std::sync::Arc;

use chrono::Utc;

use crate::{
    config::DataEnrichmentConfig,
    state::TradingState,
    workflow::{
        SnapshotStore, context_bridge::deserialize_state_from_context, snapshot::SnapshotPhase,
    },
};

use super::{run_preflight, run_preflight_with_store, test_store};

#[tokio::test]
async fn preflight_attaches_no_prior_thesis_when_store_is_empty() {
    let ctx = run_preflight("AAPL", DataEnrichmentConfig::default())
        .await
        .expect("preflight should succeed");

    let state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");

    assert!(
        state.prior_thesis.is_none(),
        "no prior snapshot means prior_thesis must be None"
    );
}

#[tokio::test]
async fn preflight_attaches_prior_thesis_when_prior_run_exists() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("preflight-thesis.db");
    let store = Arc::new(
        SnapshotStore::new(Some(&path))
            .await
            .expect("store should open"),
    );

    let mut prior_state = TradingState::new("AAPL", "2026-01-01");
    prior_state.current_thesis = Some(crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Buy".to_owned(),
        decision: "Approved".to_owned(),
        rationale: "Strong momentum.".to_owned(),
        summary: None,
        execution_id: "prior-exec-001".to_owned(),
        target_date: "2026-01-01".to_owned(),
        captured_at: Utc::now(),
    });
    store
        .save_snapshot(
            &prior_state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &prior_state,
            None,
        )
        .await
        .expect("seed prior snapshot");

    let ctx = run_preflight_with_store("AAPL", DataEnrichmentConfig::default(), store)
        .await
        .expect("preflight should succeed");

    let state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");

    let thesis = state
        .prior_thesis
        .expect("prior_thesis should be set after preflight with a prior run");
    assert_eq!(thesis.action, "Buy");
    assert_eq!(thesis.decision, "Approved");
}

#[tokio::test]
async fn preflight_prior_thesis_is_none_for_different_symbol() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("preflight-diff-symbol.db");
    let store = Arc::new(
        SnapshotStore::new(Some(&path))
            .await
            .expect("store should open"),
    );

    let mut prior_state = TradingState::new("AAPL", "2026-01-01");
    prior_state.current_thesis = Some(crate::state::ThesisMemory {
        symbol: "AAPL".to_owned(),
        action: "Buy".to_owned(),
        decision: "Approved".to_owned(),
        rationale: "Strong momentum.".to_owned(),
        summary: None,
        execution_id: "prior-aapl".to_owned(),
        target_date: "2026-01-01".to_owned(),
        captured_at: Utc::now(),
    });
    store
        .save_snapshot(
            &prior_state.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &prior_state,
            None,
        )
        .await
        .expect("seed prior snapshot");

    let ctx = run_preflight_with_store("TSLA", DataEnrichmentConfig::default(), store)
        .await
        .expect("preflight should succeed");

    let state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");

    assert!(
        state.prior_thesis.is_none(),
        "TSLA preflight must not load AAPL thesis"
    );
}

#[tokio::test]
async fn preflight_fails_closed_when_thesis_lookup_storage_fails() {
    let (store, _dir) = test_store().await;
    store.close_for_test().await;

    let result = run_preflight_with_store("AAPL", DataEnrichmentConfig::default(), store).await;

    assert!(
        result.is_err(),
        "lookup/storage failure must fail preflight"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("thesis memory lookup failed"),
        "unexpected error: {msg}"
    );
}
