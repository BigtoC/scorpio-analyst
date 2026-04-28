use super::{in_memory_store, sample_thesis};
use crate::data::adapters::{EnrichmentStatus, estimates::ConsensusEvidence};
use crate::state::EnrichmentState;
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
async fn load_prior_thesis_skips_rows_with_mismatched_schema_version() {
    // Same-version-only after the Phase 6 bump: any row whose stored
    // `schema_version` does not equal `THESIS_MEMORY_SCHEMA_VERSION` is
    // skipped before deserialization. Simulate a pre-v2 row by writing
    // `schema_version = 1`.
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
        "UPDATE phase_snapshots SET schema_version = 1 WHERE execution_id = ? AND phase_number = 5",
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
        "rows with mismatched schema_version must be skipped"
    );
}

#[tokio::test]
async fn load_prior_thesis_prefers_newest_compatible_schema_row() {
    // Seed two phase-5 rows for the same symbol: a stale v1 (incompatible)
    // row inserted first, then a newer v2 row. The lookup must return the
    // v2 thesis even though the v1 row is "newer" in insertion order only
    // if the same-version filter is working.
    let store = in_memory_store().await;
    let mut stale = TradingState::new("AAPL", "2026-04-07");
    stale.current_thesis = Some(sample_thesis());
    let stale_exec = stale.execution_id.to_string();

    store
        .save_snapshot(&stale_exec, SnapshotPhase::FundManager, &stale, None)
        .await
        .expect("stale save should succeed");

    // Backdate the stale row to a stored v1 schema_version.
    sqlx::query(
        "UPDATE phase_snapshots SET schema_version = 1 WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(&stale_exec)
    .execute(&store.pool)
    .await
    .expect("stale-version update should succeed");

    // Write a fresh v2 row (the default version on save).
    let mut fresh = TradingState::new("AAPL", "2026-04-08");
    let mut fresh_thesis = sample_thesis();
    fresh_thesis.action = "Sell".to_owned();
    fresh.current_thesis = Some(fresh_thesis);
    store
        .save_snapshot(
            &fresh.execution_id.to_string(),
            SnapshotPhase::FundManager,
            &fresh,
            None,
        )
        .await
        .expect("fresh save should succeed");

    let loaded = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed")
        .expect("v2 row should be reused");

    assert_eq!(
        loaded.action, "Sell",
        "lookup must return the v2 thesis, not the stale v1 row"
    );
}

#[tokio::test]
async fn load_prior_thesis_skips_undeserializable_payload_and_returns_none() {
    // Rows whose JSON cannot be deserialized (e.g. due to schema evolution or
    // actual corruption) are skipped with a warning rather than hard-failing the
    // pipeline, so a stale incompatible snapshot never blocks a new analysis run.
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

    assert!(
        matches!(result, Ok(None)),
        "undeserializable row should be skipped, not hard-failed: {result:?}"
    );
}

#[tokio::test]
async fn load_prior_thesis_skips_higher_schema_version_rows_after_downgrade() {
    // Reverse-direction safety: a v2 binary running against a database that
    // already contains v3 rows (e.g. an operator who upgraded, ran once, then
    // rolled back to a prior binary) must skip the v3 rows the same way a v3
    // binary skips v2 rows. The existing `!=` skip path at thesis.rs:83
    // handles both directions; this test pins that contract so a future
    // refactor that switches to `<` would fail.
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-26");
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

    // Simulate a v3-binary write by stamping the row at the *current* active
    // version + 1; the binary running this test is conceptually older.
    sqlx::query(
        "UPDATE phase_snapshots SET schema_version = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(crate::workflow::snapshot::thesis::THESIS_MEMORY_SCHEMA_VERSION + 1)
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
        "rows with newer schema_version must be skipped on read so downgrade is non-fatal"
    );
}

#[tokio::test]
async fn additive_consensus_and_technical_fields_do_not_require_schema_bump() {
    // Forward-compat regression: writing a phase-5 snapshot at the *current*
    // active schema version, then stripping the new additive keys (`url`,
    // `options_summary`, `price_target`, `recommendations`) from the stored
    // JSON, must NOT cause the loader to skip the row. These fields ride the
    // existing schema version because they're additive `Option<_>` with
    // `#[serde(default)]`.
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-26");
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

    // Pull the saved JSON back out, strip the additive keys recursively, and
    // overwrite the row with the reduced payload. The schema_version stays at
    // the current active version so the loader is forced to deserialize.
    let row: (String,) = sqlx::query_as(
        "SELECT trading_state_json FROM phase_snapshots WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(state.execution_id.to_string())
    .fetch_one(&store.pool)
    .await
    .expect("fetch saved snapshot");

    let mut value: serde_json::Value =
        serde_json::from_str(&row.0).expect("saved JSON must be valid");
    strip_keys_recursively(
        &mut value,
        &["url", "options_summary", "price_target", "recommendations"],
    );
    let stripped = serde_json::to_string(&value).expect("re-serialize stripped payload");

    sqlx::query(
        "UPDATE phase_snapshots SET trading_state_json = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(&stripped)
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("stripped-payload update should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert_eq!(
        result.as_ref().map(|t| t.action.as_str()),
        Some("Buy"),
        "additive optional fields removed from stored JSON must not block thesis lookup"
    );
}

#[tokio::test]
async fn additive_fields_deserialize_when_struct_lacks_field() {
    // Reverse-direction safety: a snapshot row stamped at the current active
    // schema version that contains an extra unknown key (e.g., a future
    // additive field) inside the trading-state JSON must still deserialize as
    // if the unknown key were absent. Without this guarantee, snapshotted
    // state structs cannot evolve additively across binary downgrades.
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-27");
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

    let row: (String,) = sqlx::query_as(
        "SELECT trading_state_json FROM phase_snapshots WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(state.execution_id.to_string())
    .fetch_one(&store.pool)
    .await
    .expect("fetch saved snapshot");

    let mut value: serde_json::Value =
        serde_json::from_str(&row.0).expect("saved JSON must be valid");
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "future_field".to_owned(),
            serde_json::json!("unknown additive field"),
        );
    }
    let augmented = serde_json::to_string(&value).expect("re-serialize augmented payload");

    sqlx::query(
        "UPDATE phase_snapshots SET trading_state_json = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(&augmented)
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("augmented-payload update should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert_eq!(
        result.as_ref().map(|t| t.action.as_str()),
        Some("Buy"),
        "unknown additive keys in stored trading-state JSON must not block thesis lookup"
    );
}

#[tokio::test]
async fn additive_options_context_field_does_not_require_schema_bump() {
    // Forward-compat regression: writing a phase-5 snapshot at the *current*
    // active schema version, then stripping the `options_context` key from the
    // stored JSON (simulating a snapshot produced before this field existed),
    // must NOT cause the loader to skip the row. The field rides the existing
    // schema version because it is additive `Option<_>` with `#[serde(default)]`.
    let store = in_memory_store().await;
    let mut state = TradingState::new("AAPL", "2026-04-29");
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

    let row: (String,) = sqlx::query_as(
        "SELECT trading_state_json FROM phase_snapshots WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(state.execution_id.to_string())
    .fetch_one(&store.pool)
    .await
    .expect("fetch saved snapshot");

    let mut value: serde_json::Value =
        serde_json::from_str(&row.0).expect("saved JSON must be valid");
    strip_keys_recursively(&mut value, &["options_context"]);
    let stripped = serde_json::to_string(&value).expect("re-serialize stripped payload");

    sqlx::query(
        "UPDATE phase_snapshots SET trading_state_json = ? WHERE execution_id = ? AND phase_number = 5",
    )
    .bind(&stripped)
    .bind(state.execution_id.to_string())
    .execute(&store.pool)
    .await
    .expect("stripped-payload update should succeed");

    let result = store
        .load_prior_thesis_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed");

    assert_eq!(
        result.as_ref().map(|t| t.action.as_str()),
        Some("Buy"),
        "additive options_context field removed from stored JSON must not block thesis lookup"
    );
}

/// Recursively delete every occurrence of `keys` from `value`, walking both
/// objects and arrays. Used by the additive-fields backward-compat tests to
/// simulate snapshots produced before a new optional field existed.
fn strip_keys_recursively(value: &mut serde_json::Value, keys: &[&str]) {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                map.remove(*key);
            }
            for v in map.values_mut() {
                strip_keys_recursively(v, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                strip_keys_recursively(item, keys);
            }
        }
        _ => {}
    }
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

#[tokio::test]
async fn load_prior_consensus_returns_latest_phase_one_payload_for_symbol() {
    let store = in_memory_store().await;

    let mut stale = TradingState::new("AAPL", "2026-04-26");
    stale.enrichment_consensus = EnrichmentState {
        status: EnrichmentStatus::FetchFailed("provider_degraded".to_owned()),
        payload: Some(ConsensusEvidence {
            symbol: "AAPL".to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2026-04-26".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: 1,
        }),
    };
    store
        .save_snapshot(
            &stale.execution_id.to_string(),
            SnapshotPhase::AnalystTeam,
            &stale,
            None,
        )
        .await
        .expect("save stale analyst-team snapshot");

    let mut fresh = TradingState::new("AAPL", "2026-04-27");
    fresh.enrichment_consensus = EnrichmentState {
        status: EnrichmentStatus::FetchFailed("provider_degraded".to_owned()),
        payload: Some(ConsensusEvidence {
            symbol: "AAPL".to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2026-04-27".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: 2,
        }),
    };
    store
        .save_snapshot(
            &fresh.execution_id.to_string(),
            SnapshotPhase::AnalystTeam,
            &fresh,
            None,
        )
        .await
        .expect("save fresh analyst-team snapshot");

    let loaded = store
        .load_prior_consensus_for_symbol("AAPL", 30)
        .await
        .expect("query should succeed")
        .expect("latest phase-1 consensus payload should be found");

    assert_eq!(loaded.as_of_date, "2026-04-27");
    assert_eq!(loaded.consecutive_provider_degraded_cycles, 2);
}
