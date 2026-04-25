//! Phase snapshot persistence using SQLite.
//!
//! [`SnapshotStore`] saves and loads immutable point-in-time snapshots of
//! [`TradingState`] for each workflow [`SnapshotPhase`]. The SQLite database is
//! stored at `$HOME/.scorpio-analyst/phase_snapshots.db` by default; callers
//! may override this by passing an explicit path to [`SnapshotStore::new`].
//! [`SnapshotStore::load_snapshot`] returns a [`LoadedSnapshot`] with named fields
//! instead of a positional tuple.
//! Snapshot setup failures from [`SnapshotStore::new`] return
//! [`TradingError::Config`], while snapshot save/load runtime and payload failures
//! return [`TradingError::Storage`].

use std::path::Path;

use anyhow::Context as _;
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::debug;

use self::path::resolve_db_path;

use crate::{
    config::Config,
    error::TradingError,
    state::{AgentTokenUsage, TradingState},
};

mod path;
mod thesis;

pub(crate) use thesis::THESIS_MEMORY_SCHEMA_VERSION;

#[cfg(test)]
mod tests;

/// Named workflow phases that can be persisted as snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotPhase {
    AnalystTeam,
    ResearcherDebate,
    Trader,
    RiskDiscussion,
    FundManager,
}

impl SnapshotPhase {
    /// Return the persisted phase number used as the storage key.
    pub const fn number(self) -> u8 {
        match self {
            Self::AnalystTeam => 1,
            Self::ResearcherDebate => 2,
            Self::Trader => 3,
            Self::RiskDiscussion => 4,
            Self::FundManager => 5,
        }
    }

    /// Return the stable phase name stored alongside the snapshot.
    pub const fn name(self) -> &'static str {
        match self {
            Self::AnalystTeam => "analyst_team",
            Self::ResearcherDebate => "researcher_debate",
            Self::Trader => "trader",
            Self::RiskDiscussion => "risk_discussion",
            Self::FundManager => "fund_manager",
        }
    }
}

/// Loaded snapshot payload with named fields.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSnapshot {
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
}

/// Manages SQLite-backed phase-snapshot persistence for a trading pipeline run.
#[derive(Debug)]
pub struct SnapshotStore {
    pool: SqlitePool,
}

impl SnapshotStore {
    /// Open (or create) the snapshot store configured for this application.
    ///
    /// Uses [`crate::config::StorageConfig::snapshot_db_path`] after applying
    /// the project's `~/` / `$HOME/` expansion rules.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`SnapshotStore::new`] if path resolution,
    /// directory creation, or SQLite initialization fails.
    pub async fn from_config(config: &Config) -> Result<Self, TradingError> {
        let db_path = crate::config::expand_path(&config.storage.snapshot_db_path);
        Self::new(Some(&db_path)).await
    }

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
        debug!(path = %resolved.display(), "opening phase snapshot store");

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
    /// Returns [`TradingError::Storage`] for snapshot serialization and database
    /// failures that occur at runtime after the store is configured.
    pub async fn save_snapshot(
        &self,
        execution_id: &str,
        phase: SnapshotPhase,
        state: &TradingState,
        token_usage: Option<&[AgentTokenUsage]>,
    ) -> Result<(), TradingError> {
        let phase_number = phase.number();
        let phase_name = phase.name();

        let state_json = serialize_snapshot_json(state, "TradingState")?;

        let usage_json = token_usage
            .map(|u| serialize_snapshot_json(u, "token usage"))
            .transpose()?;

        let created_at = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO phase_snapshots
                (execution_id, phase_number, phase_name, trading_state_json, token_usage_json, created_at, symbol, schema_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(execution_id, phase_number) DO UPDATE SET
                phase_name          = excluded.phase_name,
                trading_state_json  = excluded.trading_state_json,
                token_usage_json    = excluded.token_usage_json,
                created_at          = excluded.created_at,
                symbol              = excluded.symbol,
                schema_version      = excluded.schema_version",
        )
        .bind(execution_id)
        .bind(phase_number as i64)
        .bind(phase_name)
        .bind(&state_json)
        .bind(usage_json.as_deref())
        .bind(&created_at)
        .bind(&state.asset_symbol)
        .bind(THESIS_MEMORY_SCHEMA_VERSION)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to save snapshot phase={phase_number} exec={execution_id}"))
        .map_err(TradingError::Storage)?;

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

    /// Load a phase snapshot by `execution_id` and [`SnapshotPhase`].
    ///
    /// Returns `Ok(None)` if no matching row exists.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::Storage`] on database failures and snapshot payload
    /// decode failures that occur at runtime.
    pub async fn load_snapshot(
        &self,
        execution_id: &str,
        phase: SnapshotPhase,
    ) -> Result<Option<LoadedSnapshot>, TradingError> {
        let phase_number = phase.number();

        let row: Option<(Option<i64>, String, Option<String>)> = sqlx::query_as(
            "SELECT schema_version, trading_state_json, token_usage_json
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
        .map_err(TradingError::Storage)?;

        match row {
            None => Ok(None),
            Some((schema_version, state_json, usage_json)) => {
                // Same-version-only after the Phase 6 bump: a pre-v2 row is
                // unsupported after the `TradingState` equity sub-state reshape.
                // Surface the mismatch as a typed storage error so callers can
                // fail fast rather than attempt a deserialization that would
                // lose or misplace fields.
                let schema_version = schema_version.unwrap_or(0);
                if schema_version != THESIS_MEMORY_SCHEMA_VERSION {
                    return Err(TradingError::Storage(anyhow::anyhow!(
                        "incompatible snapshot schema_version={schema_version} \
                         (active={THESIS_MEMORY_SCHEMA_VERSION}) for exec={execution_id} \
                         phase={phase_number}"
                    )));
                }

                let state: TradingState = serde_json::from_str(&state_json)
                    .with_context(|| "failed to deserialize TradingState from snapshot")
                    .map_err(TradingError::Storage)?;

                let usage = usage_json
                    .map(|json| {
                        serde_json::from_str::<Vec<AgentTokenUsage>>(&json)
                            .with_context(|| "failed to deserialize token usage from snapshot")
                            .map_err(TradingError::Storage)
                    })
                    .transpose()?;

                Ok(Some(LoadedSnapshot {
                    state,
                    token_usage: usage,
                }))
            }
        }
    }
}

fn serialize_snapshot_json<T: Serialize + ?Sized>(
    value: &T,
    label: &str,
) -> Result<String, TradingError> {
    serde_json::to_string(value)
        .with_context(|| format!("failed to serialize {label} for snapshot"))
        .map_err(TradingError::Storage)
}
