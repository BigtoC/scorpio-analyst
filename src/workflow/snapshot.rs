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

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use tracing::debug;

use crate::{
    config::Config,
    error::TradingError,
    state::{AgentTokenUsage, ThesisMemory, TradingState},
};

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
        .bind(1_i64)
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

    /// Load the most recent prior thesis for a canonical symbol.
    ///
    /// Queries phase-5 snapshots for `symbol` that are no older than
    /// `max_age_days`.  Returns the `current_thesis` field from the most recent
    /// matching snapshot's `TradingState`, or `None` if no compatible snapshot
    /// exists.
    ///
    /// Snapshots whose `trading_state_json` fails to deserialize are skipped and
    /// logged rather than surfaced as hard errors, consistent with the plan's
    /// fail-open semantics for missing prior memory.  Actual storage/connection
    /// failures remain hard errors (returned as [`TradingError::Storage`]).
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::Storage`] on database connection or query failures.
    pub async fn load_prior_thesis_for_symbol(
        &self,
        symbol: &str,
        max_age_days: i64,
    ) -> Result<Option<ThesisMemory>, TradingError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_age_days)).to_rfc3339();

        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT trading_state_json
             FROM phase_snapshots
             WHERE symbol = ? AND phase_number = 5 AND created_at >= ?
             ORDER BY created_at DESC",
        )
        .bind(symbol)
        .bind(&cutoff)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("failed to query prior-thesis snapshots for symbol={symbol}"))
        .map_err(TradingError::Storage)?;

        for (state_json,) in rows {
            match serde_json::from_str::<TradingState>(&state_json) {
                Ok(state) => {
                    if let Some(thesis) = state.current_thesis {
                        return Ok(Some(thesis));
                    }
                    // Phase-5 snapshot exists but has no current_thesis (e.g. from
                    // a run before thesis memory was introduced). Skip and continue.
                    debug!(
                        symbol,
                        "prior phase-5 snapshot has no current_thesis; skipping"
                    );
                }
                Err(e) => {
                    // Deserialization failed — log and skip this snapshot rather than
                    // propagating the error, because missing prior memory is fail-open.
                    debug!(symbol, error = %e, "prior-thesis snapshot failed to deserialize; skipping");
                }
            }
        }

        Ok(None)
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
        .map_err(TradingError::Storage)?;

        match row {
            None => Ok(None),
            Some((state_json, usage_json)) => {
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

/// Resolve the SQLite database path.
///
/// If `db_path` is `Some`, basic validation is applied to reject clearly unsafe
/// or malformed inputs (empty paths, embedded null bytes, bare path-traversal
/// sequences).  Otherwise the default `$HOME/.scorpio-analyst/phase_snapshots.db`
/// is returned.
fn resolve_db_path(db_path: Option<&Path>) -> Result<PathBuf, TradingError> {
    if let Some(p) = db_path {
        let s = p.to_string_lossy();

        // Reject empty paths.
        if s.is_empty() {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not be empty"
            )));
        }

        // Reject embedded null bytes (would truncate the path in C-based libs
        // like SQLite).
        if s.contains('\0') {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not contain null bytes"
            )));
        }

        // Reject paths that are *purely* traversal sequences (e.g. "../../../"
        // or "..").  We do NOT reject paths like "/legit/dir/../file.db" because
        // those are normal relative references; we only block paths whose
        // *every* component is `.` or `..`, which have no meaningful file
        // destination.
        let all_traversal = p.components().all(|c| {
            matches!(
                c,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        });
        if all_traversal {
            return Err(TradingError::Config(anyhow::anyhow!(
                "snapshot db_path must not be a bare traversal path: {s}"
            )));
        }

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
    use crate::error::TradingError;
    use crate::state::{
        DataCoverageReport, EvidenceKind, EvidenceRecord, EvidenceSource, FundamentalData,
        ProvenanceSummary, TradingState,
    };
    use chrono::Utc;

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

    #[derive(Debug)]
    struct FailingSerialize;

    impl serde::Serialize for FailingSerialize {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom(
                "intentional serialization failure",
            ))
        }
    }

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

    #[test]
    fn storage_error_preserves_source() {
        let error = TradingError::Storage(anyhow::anyhow!("snapshot failed"));
        assert!(std::error::Error::source(&error).is_some());
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

        // Modify state and save again under the same phase.
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

        // Should reflect the updated state.
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
    async fn save_snapshot_uses_typed_phase_api() {
        let store = in_memory_store().await;
        let state = sample_state();

        store
            .save_snapshot(
                &state.execution_id.to_string(),
                SnapshotPhase::Trader,
                &state,
                None,
            )
            .await
            .expect("typed phase save should succeed");

        let loaded = store
            .load_snapshot(&state.execution_id.to_string(), SnapshotPhase::Trader)
            .await
            .expect("load should succeed")
            .expect("snapshot should exist");

        assert_eq!(loaded.state.asset_symbol, state.asset_symbol);
    }

    #[test]
    fn serialize_snapshot_json_returns_storage_error_for_serialization_failures() {
        let error = serialize_snapshot_json(&FailingSerialize, "failing value")
            .expect_err("intentional serializer failure should propagate");

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

    #[tokio::test]
    async fn snapshot_store_implements_debug() {
        let store = in_memory_store().await;
        let rendered = format!("{store:?}");
        assert!(rendered.contains("SnapshotStore"));
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
    async fn from_config_uses_expanded_snapshot_db_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("configured.db");
        let mut config = crate::config::Config::load_from("config.toml").expect("config load");
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

    // ── Path-validation edge cases ───────────────────────────────────────

    #[test]
    fn empty_path_is_rejected() {
        let result = resolve_db_path(Some(Path::new("")));
        assert!(result.is_err(), "empty path should be rejected");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("must not be empty"),
            "error should mention empty: {msg}"
        );
    }

    #[test]
    fn null_byte_path_is_rejected() {
        let result = resolve_db_path(Some(Path::new("/tmp/bad\0.db")));
        assert!(result.is_err(), "null-byte path should be rejected");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("null bytes"),
            "error should mention null bytes: {msg}"
        );
    }

    #[test]
    fn bare_traversal_path_is_rejected() {
        let result = resolve_db_path(Some(Path::new("../../..")));
        assert!(result.is_err(), "bare traversal path should be rejected");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("bare traversal"),
            "error should mention traversal: {msg}"
        );
    }

    #[test]
    fn dot_only_path_is_rejected() {
        let result = resolve_db_path(Some(Path::new(".")));
        assert!(result.is_err(), "bare '.' path should be rejected");
    }

    #[test]
    fn legitimate_path_with_parent_ref_is_accepted() {
        // Paths like "/tmp/foo/../bar.db" are legitimate relative refs.
        let p = Path::new("/tmp/foo/../bar.db");
        let resolved = resolve_db_path(Some(p)).expect("should resolve");
        assert_eq!(resolved, p);
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

    #[tokio::test]
    async fn evidence_fields_survive_snapshot_round_trip() {
        let store = in_memory_store().await;
        let mut state = TradingState::new("TSLA", "2026-01-15");
        let exec_id = state.execution_id.to_string();

        // Populate typed evidence fields.
        state.evidence_fundamental = Some(EvidenceRecord {
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
            loaded.state.evidence_fundamental.is_some(),
            "evidence_fundamental must survive snapshot"
        );
        assert_eq!(
            loaded
                .state
                .evidence_fundamental
                .as_ref()
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

    // ── Thesis memory snapshot tests ─────────────────────────────────────────

    fn sample_thesis() -> crate::state::ThesisMemory {
        crate::state::ThesisMemory {
            symbol: "AAPL".to_owned(),
            action: "Buy".to_owned(),
            decision: "Approved".to_owned(),
            rationale: "Strong fundamentals.".to_owned(),
            summary: None,
            execution_id: "exec-thesis-001".to_owned(),
            target_date: "2026-04-07".to_owned(),
            captured_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn load_prior_thesis_returns_none_when_no_prior_snapshot() {
        let store = in_memory_store().await;

        let result = store
            .load_prior_thesis_for_symbol("AAPL", 30)
            .await
            .expect("query should succeed");

        assert!(result.is_none(), "no prior snapshot should yield None");
    }

    #[tokio::test]
    async fn load_prior_thesis_returns_none_when_no_phase5_snapshot() {
        let store = in_memory_store().await;
        let mut state = TradingState::new("AAPL", "2026-04-07");
        state.current_thesis = Some(sample_thesis());
        let exec_id = state.execution_id.to_string();

        // Save only a phase-1 snapshot (not phase-5)
        store
            .save_snapshot(&exec_id, SnapshotPhase::AnalystTeam, &state, None)
            .await
            .expect("save should succeed");

        let result = store
            .load_prior_thesis_for_symbol("AAPL", 30)
            .await
            .expect("query should succeed");

        assert!(
            result.is_none(),
            "no phase-5 snapshot means no prior thesis"
        );
    }

    #[tokio::test]
    async fn load_prior_thesis_returns_thesis_from_phase5_snapshot() {
        let store = in_memory_store().await;
        let mut state = TradingState::new("AAPL", "2026-04-07");
        let thesis = sample_thesis();
        state.current_thesis = Some(thesis.clone());
        let exec_id = state.execution_id.to_string();

        store
            .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
            .await
            .expect("save should succeed");

        let result = store
            .load_prior_thesis_for_symbol("AAPL", 30)
            .await
            .expect("query should succeed");

        let loaded_thesis = result.expect("prior thesis should be found");
        assert_eq!(loaded_thesis.symbol, "AAPL");
        assert_eq!(loaded_thesis.action, "Buy");
        assert_eq!(loaded_thesis.decision, "Approved");
        assert_eq!(loaded_thesis.rationale, "Strong fundamentals.");
    }

    #[tokio::test]
    async fn load_prior_thesis_returns_most_recent_when_multiple_runs() {
        let store = in_memory_store().await;

        // Save an older run
        let mut old_state = TradingState::new("AAPL", "2026-01-01");
        old_state.current_thesis = Some(crate::state::ThesisMemory {
            symbol: "AAPL".to_owned(),
            action: "Hold".to_owned(),
            decision: "Rejected".to_owned(),
            rationale: "Old rationale.".to_owned(),
            summary: None,
            execution_id: "exec-old".to_owned(),
            target_date: "2026-01-01".to_owned(),
            captured_at: Utc::now() - chrono::Duration::hours(2),
        });
        store
            .save_snapshot(
                &old_state.execution_id.to_string(),
                SnapshotPhase::FundManager,
                &old_state,
                None,
            )
            .await
            .expect("save old run");
        sqlx::query(
            "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
        )
        .bind((Utc::now() - chrono::Duration::hours(2)).to_rfc3339())
        .bind(old_state.execution_id.to_string())
        .execute(&store.pool)
        .await
        .expect("timestamp update for old run");

        // Save a newer run
        let mut new_state = TradingState::new("AAPL", "2026-04-07");
        new_state.current_thesis = Some(crate::state::ThesisMemory {
            symbol: "AAPL".to_owned(),
            action: "Buy".to_owned(),
            decision: "Approved".to_owned(),
            rationale: "New rationale.".to_owned(),
            summary: None,
            execution_id: "exec-new".to_owned(),
            target_date: "2026-04-07".to_owned(),
            captured_at: Utc::now() - chrono::Duration::hours(1),
        });
        store
            .save_snapshot(
                &new_state.execution_id.to_string(),
                SnapshotPhase::FundManager,
                &new_state,
                None,
            )
            .await
            .expect("save new run");
        sqlx::query(
            "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
        )
        .bind((Utc::now() - chrono::Duration::hours(1)).to_rfc3339())
        .bind(new_state.execution_id.to_string())
        .execute(&store.pool)
        .await
        .expect("timestamp update for new run");

        let result = store
            .load_prior_thesis_for_symbol("AAPL", 30)
            .await
            .expect("query should succeed");

        let thesis = result.expect("should find prior thesis");
        assert_eq!(thesis.action, "Buy", "newest thesis must win");
        assert_eq!(thesis.rationale, "New rationale.");
    }

    #[tokio::test]
    async fn load_prior_thesis_checks_beyond_five_ineligible_recent_rows() {
        let store = in_memory_store().await;

        for i in 0..5 {
            let state = TradingState::new("AAPL", format!("2026-04-0{}", i + 1));
            store
                .save_snapshot(
                    &state.execution_id.to_string(),
                    SnapshotPhase::FundManager,
                    &state,
                    None,
                )
                .await
                .expect("save ineligible run");
        }

        let mut eligible_state = TradingState::new("AAPL", "2026-04-07");
        eligible_state.current_thesis = Some(crate::state::ThesisMemory {
            symbol: "AAPL".to_owned(),
            action: "Buy".to_owned(),
            decision: "Approved".to_owned(),
            rationale: "Eligible older thesis.".to_owned(),
            summary: None,
            execution_id: "exec-eligible".to_owned(),
            target_date: "2026-04-07".to_owned(),
            captured_at: Utc::now() - chrono::Duration::hours(6),
        });
        store
            .save_snapshot(
                &eligible_state.execution_id.to_string(),
                SnapshotPhase::FundManager,
                &eligible_state,
                None,
            )
            .await
            .expect("save eligible run");
        sqlx::query(
            "UPDATE phase_snapshots SET created_at = ? WHERE execution_id = ? AND phase_number = 5",
        )
        .bind((Utc::now() - chrono::Duration::hours(6)).to_rfc3339())
        .bind(eligible_state.execution_id.to_string())
        .execute(&store.pool)
        .await
        .expect("timestamp update for eligible run");

        let thesis = store
            .load_prior_thesis_for_symbol("AAPL", 30)
            .await
            .expect("query should succeed")
            .expect("eligible thesis should still be found");

        assert_eq!(thesis.action, "Buy");
        assert_eq!(thesis.rationale, "Eligible older thesis.");
    }

    #[tokio::test]
    async fn load_prior_thesis_returns_none_for_different_symbol() {
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

        let result = store
            .load_prior_thesis_for_symbol("TSLA", 30)
            .await
            .expect("query should succeed");

        assert!(
            result.is_none(),
            "TSLA lookup should not return AAPL thesis"
        );
    }

    #[tokio::test]
    async fn load_prior_thesis_skips_snapshots_without_current_thesis() {
        let store = in_memory_store().await;

        // Save a phase-5 snapshot WITHOUT current_thesis (simulates pre-thesis-memory run)
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
    async fn save_snapshot_persists_symbol_column() {
        let store = in_memory_store().await;
        let state = TradingState::new("MSFT", "2026-04-07");
        let exec_id = state.execution_id.to_string();

        store
            .save_snapshot(&exec_id, SnapshotPhase::FundManager, &state, None)
            .await
            .expect("save should succeed");

        // Verify symbol was stored by performing a symbol-based lookup
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM phase_snapshots WHERE symbol = ? AND phase_number = 5",
        )
        .bind("MSFT")
        .fetch_one(&store.pool)
        .await
        .expect("count query should succeed");

        assert_eq!(count.0, 1, "one phase-5 snapshot for MSFT should exist");
    }
}
