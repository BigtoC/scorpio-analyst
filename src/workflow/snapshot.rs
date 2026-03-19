//! Phase snapshot persistence using SQLite.
//!
//! [`SnapshotStore`] saves and loads immutable point-in-time snapshots of
//! [`TradingState`] for each of the 5 pipeline phases.  The SQLite database is
//! stored at `$HOME/.scorpio-analyst/phase_snapshots.db` by default; callers
//! may override this by passing an explicit path to [`SnapshotStore::new`].

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use sqlx::SqlitePool;
use tracing::{debug, info};

use crate::{
    error::TradingError,
    state::{AgentTokenUsage, TradingState},
};

/// Manages SQLite-backed phase-snapshot persistence for a trading pipeline run.
pub struct SnapshotStore {
    pool: SqlitePool,
}

impl SnapshotStore {
    /// Open (or create) the snapshot store at the given path.
    ///
    /// If `db_path` is `None`, the default path
    /// `$HOME/.scorpio-analyst/phase_snapshots.db` is used.  The parent directory
    /// is created automatically if absent.
    ///
    /// The inline migration (creating the `phase_snapshots` table) is executed on
    /// every open so the schema is always up to date.
    ///
    /// # Errors
    ///
    /// Returns `TradingError::Config` if the home directory cannot be resolved, the
    /// parent directory cannot be created, or the SQLite pool cannot be opened.
    pub async fn new(db_path: Option<&Path>) -> Result<Self, TradingError> {
        let resolved = resolve_db_path(db_path)?;

        // Ensure the parent directory exists.
        if let Some(parent) = resolved.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))
                .map_err(TradingError::Config)?;
        }

        let db_url = format!("sqlite://{}?mode=rwc", resolved.display());
        info!(path = %resolved.display(), "opening phase snapshot store");

        let pool = SqlitePool::connect(&db_url)
            .await
            .with_context(|| format!("failed to open SQLite pool at {}", resolved.display()))
            .map_err(TradingError::Config)?;

        // Run migrations from the `migrations/` directory (path relative to crate root).
        sqlx::migrate!()
            .run(&pool)
            .await
            .with_context(|| "failed to run phase_snapshots migration")
            .map_err(TradingError::Config)?;

        Ok(Self { pool })
    }

    /// Save a phase snapshot (upsert semantics — replaces an existing row for the
    /// same `(execution_id, phase_number)` pair).
    ///
    /// # Errors
    ///
    /// Returns `TradingError::Config` on serialization or database errors.
    pub async fn save_snapshot(
        &self,
        execution_id: &str,
        phase_number: u8,
        phase_name: &str,
        state: &TradingState,
        token_usage: Option<&[AgentTokenUsage]>,
    ) -> Result<(), TradingError> {
        let state_json = serde_json::to_string(state)
            .with_context(|| "failed to serialize TradingState for snapshot")
            .map_err(TradingError::Config)?;

        let usage_json = token_usage
            .map(|u| {
                serde_json::to_string(u)
                    .with_context(|| "failed to serialize token usage for snapshot")
                    .map_err(TradingError::Config)
            })
            .transpose()?;

        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(execution_id, phase_number) DO UPDATE SET
                phase_name          = excluded.phase_name,
                trading_state_json  = excluded.trading_state_json,
                token_usage_json    = excluded.token_usage_json,
                created_at          = excluded.created_at",
        )
        .bind(execution_id)
        .bind(phase_number as i64)
        .bind(phase_name)
        .bind(&state_json)
        .bind(usage_json.as_deref())
        .bind(&created_at)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to save snapshot phase={phase_number} exec={execution_id}"))
        .map_err(TradingError::Config)?;

        debug!(
            execution_id,
            phase_number, phase_name, "phase snapshot saved"
        );
        Ok(())
    }

    /// Close the underlying connection pool.
    ///
    /// For use in unit tests only — calling this makes all subsequent save/load
    /// operations fail with a pool-closed error, which lets tests verify that
    /// snapshot failures propagate as `Err` out of workflow tasks.
    #[cfg(test)]
    pub(crate) async fn close_for_test(&self) {
        self.pool.close().await;
    }

    /// Load a phase snapshot by `execution_id` and `phase_number`.
    ///
    /// Returns `Ok(None)` if no matching row exists.
    ///
    /// # Errors
    ///
    /// Returns `TradingError::Config` on database errors or deserialization
    /// failures.
    pub async fn load_snapshot(
        &self,
        execution_id: &str,
        phase_number: u8,
    ) -> Result<Option<(TradingState, Option<Vec<AgentTokenUsage>>)>, TradingError> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT trading_state_json, token_usage_json
             FROM phase_snapshots
             WHERE execution_id = ? AND phase_number = ?",
        )
        .bind(execution_id)
        .bind(phase_number as i64)
        .fetch_optional(&self.pool)
        .await
        .with_context(|| {
            format!("failed to load snapshot phase={phase_number} exec={execution_id}")
        })
        .map_err(TradingError::Config)?;

        match row {
            None => Ok(None),
            Some((state_json, usage_json)) => {
                let state: TradingState = serde_json::from_str(&state_json)
                    .with_context(|| "failed to deserialize TradingState from snapshot")
                    .map_err(TradingError::Config)?;

                let usage = usage_json
                    .map(|json| {
                        serde_json::from_str::<Vec<AgentTokenUsage>>(&json)
                            .with_context(|| "failed to deserialize token usage from snapshot")
                            .map_err(TradingError::Config)
                    })
                    .transpose()?;

                Ok(Some((state, usage)))
            }
        }
    }
}

/// Resolve the SQLite database path.
///
/// If `db_path` is `Some`, it is used as-is.  Otherwise the default
/// `$HOME/.scorpio-analyst/phase_snapshots.db` is returned.
fn resolve_db_path(db_path: Option<&Path>) -> Result<PathBuf, TradingError> {
    if let Some(p) = db_path {
        return Ok(p.to_path_buf());
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .with_context(|| "HOME/USERPROFILE environment variable not set; cannot resolve default snapshot path")
        .map_err(TradingError::Config)?;

    Ok(PathBuf::from(home)
        .join(".scorpio-analyst")
        .join("phase_snapshots.db"))
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TradingState;

    /// Open an in-memory SQLite snapshot store for tests.
    async fn in_memory_store() -> SnapshotStore {
        // SQLx in-memory: use a named shared-cache URI so the same DB is accessible
        // through the pool, or simply use the file-based mode=rwc with :memory: path.
        // SQLite doesn't natively support sqlite::memory: with file URI. Use a temp file.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.keep().join("test.db");
        SnapshotStore::new(Some(&path))
            .await
            .expect("in-memory store")
    }

    fn sample_state() -> TradingState {
        TradingState::new("AAPL", "2026-01-15")
    }

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let store = in_memory_store().await;
        let state = sample_state();
        let exec_id = state.execution_id.to_string();

        store
            .save_snapshot(&exec_id, 1, "analyst_sync", &state, None)
            .await
            .expect("save should succeed");

        let loaded = store
            .load_snapshot(&exec_id, 1)
            .await
            .expect("load should succeed")
            .expect("snapshot should exist");

        assert_eq!(loaded.0.asset_symbol, state.asset_symbol);
        assert_eq!(loaded.0.target_date, state.target_date);
        assert!(loaded.1.is_none());
    }

    #[tokio::test]
    async fn upsert_replaces_existing_snapshot() {
        let store = in_memory_store().await;
        let mut state = sample_state();
        let exec_id = state.execution_id.to_string();

        store
            .save_snapshot(&exec_id, 1, "phase_one_v1", &state, None)
            .await
            .unwrap();

        // Modify state and save again under the same phase.
        state.target_date = "2026-03-19".to_string();
        store
            .save_snapshot(&exec_id, 1, "phase_one_v2", &state, None)
            .await
            .unwrap();

        let loaded = store
            .load_snapshot(&exec_id, 1)
            .await
            .unwrap()
            .expect("snapshot should exist");

        // Should reflect the updated state.
        assert_eq!(loaded.0.target_date, "2026-03-19");
    }

    #[tokio::test]
    async fn missing_snapshot_returns_none() {
        let store = in_memory_store().await;

        let result = store
            .load_snapshot("non-existent-id", 99)
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
        }];

        store
            .save_snapshot(&exec_id, 1, "analyst_sync", &state, Some(&usage))
            .await
            .unwrap();

        let (_, loaded_usage) = store
            .load_snapshot(&exec_id, 1)
            .await
            .unwrap()
            .expect("snapshot should exist");

        let loaded_usage = loaded_usage.expect("token usage should be present");
        assert_eq!(loaded_usage.len(), 1);
        assert_eq!(loaded_usage[0].agent_name, "FundamentalAnalyst");
        assert_eq!(loaded_usage[0].total_tokens, 150);
    }

    #[test]
    fn default_path_resolves_to_expected_location() {
        let path = resolve_db_path(None).expect("should resolve");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(".scorpio-analyst"),
            "expected .scorpio-analyst in path, got: {path_str}"
        );
        assert!(
            path_str.ends_with("phase_snapshots.db"),
            "expected phase_snapshots.db at end, got: {path_str}"
        );
    }

    #[test]
    fn custom_path_overrides_default() {
        let custom = Path::new("/tmp/custom_test.db");
        let resolved = resolve_db_path(Some(custom)).expect("should resolve");
        assert_eq!(resolved, custom);
    }

    #[tokio::test]
    async fn parent_directory_is_created() {
        let dir = tempfile::tempdir().expect("temp dir");
        let nested = dir.path().join("nested").join("deep").join("snap.db");
        // The directory does not exist yet.
        assert!(!nested.parent().unwrap().exists());

        SnapshotStore::new(Some(&nested))
            .await
            .expect("store should be created with auto-mkdir");

        assert!(nested.parent().unwrap().exists());
    }
}
