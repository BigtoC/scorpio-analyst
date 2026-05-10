use std::collections::{HashMap, HashSet};

use anyhow::Context as _;
use chrono::Utc;
use serde::Serialize;
use tracing::warn;

use super::{SnapshotStore, THESIS_MEMORY_SCHEMA_VERSION};
use crate::{
    error::TradingError,
    state::{AgentTokenUsage, TradingState},
};

/// Summary of a single execution for list display.
///
/// Only includes executions whose snapshot rows match the active
/// `THESIS_MEMORY_SCHEMA_VERSION`; runs from older schemas are not visible.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub execution_id: String,
    pub symbol: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Result of `list_executions` — visible summaries plus a count of stale
/// executions filtered out by the schema-version check.
///
/// Callers should surface `stale_count` so users notice when a version bump
/// has retired previously-visible runs.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionListing {
    pub summaries: Vec<ExecutionSummary>,
    pub stale_count: usize,
    pub invalid_timestamp_count: usize,
}

/// Per-phase snapshot returned by `load_full_report`.
///
/// Distinct from `super::LoadedSnapshot` because the multi-row result needs to
/// track which phase each row corresponds to.
#[derive(Debug, Clone)]
pub struct LoadedReportSnapshot {
    pub state: TradingState,
    pub token_usage: Option<Vec<AgentTokenUsage>>,
    pub phase_number: i64,
}

/// Result of `load_full_report` — visible per-phase snapshots plus a list of
/// phase numbers that were soft-skipped due to deserialization failure.
///
/// Callers should surface `skipped_phases` so corrupt rows are visible to
/// users instead of only appearing in `tracing::warn!` logs.
#[derive(Debug, Clone)]
pub struct LoadedReport {
    pub snapshots: Vec<LoadedReportSnapshot>,
    pub skipped_phases: Vec<i64>,
}

impl SnapshotStore {
    /// List all past executions visible to the current binary.
    ///
    /// Returns visible summaries plus a count of executions filtered out by the
    /// schema-version check. Visible rows are ordered by latest activity after
    /// parsing the persisted timestamp strings. Only rows whose `schema_version`
    /// matches the active `THESIS_MEMORY_SCHEMA_VERSION` are returned in
    /// `summaries`; the rest are tallied into `stale_count` so the CLI can
    /// surface a stderr banner instead of letting the user think the database is
    /// empty.
    ///
    /// Ordering note: `MAX(created_at)` reflects the latest phase save. For a
    /// completed run this is the FundManager save time; for a crashed run it is
    /// the time of the failing phase. If "started at" semantics are needed later,
    /// add a `started_at` field populated from `MIN(created_at)`.
    pub async fn list_executions(&self) -> Result<ExecutionListing, TradingError> {
        let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT execution_id, symbol, created_at
             FROM phase_snapshots
             WHERE schema_version = ?",
        )
        .bind(THESIS_MEMORY_SCHEMA_VERSION)
        .fetch_all(&self.pool)
        .await
        .context("failed to list executions")
        .map_err(TradingError::Storage)?;

        let stale_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)
             FROM (
                 SELECT execution_id
                 FROM phase_snapshots
                 GROUP BY execution_id
                 HAVING MAX(CASE WHEN schema_version = ? THEN 1 ELSE 0 END) = 0
             )",
        )
        .bind(THESIS_MEMORY_SCHEMA_VERSION)
        .fetch_one(&self.pool)
        .await
        .context("failed to count stale executions")
        .map_err(TradingError::Storage)?;

        let mut invalid_execution_ids = HashSet::new();
        let mut latest_valid_by_execution = HashMap::new();

        for (execution_id, symbol, created_at_raw) in rows {
            let Ok(created_at) = parse_snapshot_timestamp(&created_at_raw) else {
                invalid_execution_ids.insert(execution_id.clone());
                latest_valid_by_execution.remove(&execution_id);
                continue;
            };

            if invalid_execution_ids.contains(&execution_id) {
                continue;
            }

            latest_valid_by_execution
                .entry(execution_id)
                .and_modify(|(current_symbol, current_created_at)| {
                    if created_at > *current_created_at {
                        *current_symbol = symbol.clone();
                        *current_created_at = created_at;
                    }
                })
                .or_insert((symbol, created_at));
        }

        let invalid_timestamp_count = invalid_execution_ids.len();

        let mut summaries = latest_valid_by_execution
            .into_iter()
            .map(|(execution_id, (symbol, created_at))| ExecutionSummary {
                execution_id,
                symbol,
                created_at,
            })
            .collect::<Vec<_>>();

        summaries.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.execution_id.cmp(&left.execution_id))
        });

        Ok(ExecutionListing {
            summaries,
            stale_count: usize::try_from(stale_count.0).unwrap_or(0),
            invalid_timestamp_count,
        })
    }

    /// Return whether any row exists for `execution_id`, regardless of schema version.
    pub async fn execution_exists(&self, execution_id: &str) -> Result<bool, TradingError> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM phase_snapshots WHERE execution_id = ?")
                .bind(execution_id)
                .fetch_one(&self.pool)
                .await
                .with_context(|| {
                    format!("failed to check snapshot existence for execution_id={execution_id}")
                })
                .map_err(TradingError::Storage)?;

        Ok(count.0 > 0)
    }

    /// Load all phase snapshots for a given execution ID, scoped to the active schema.
    ///
    /// Returns visible snapshots ordered by phase_number ascending plus a list of
    /// phase numbers that were soft-skipped due to deserialization failure. Rows
    /// from older schema versions are filtered at the SQL boundary — they are
    /// intentionally retired data, not "missing" data. An execution whose rows
    /// are all stale will appear as not-found to the caller.
    ///
    /// A failure of `token_usage_json` degrades only that phase's `token_usage` to
    /// `None`; the snapshot is still returned (not added to `skipped_phases`).
    pub async fn load_full_report(&self, execution_id: &str) -> Result<LoadedReport, TradingError> {
        let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT phase_number, trading_state_json, token_usage_json
             FROM phase_snapshots
             WHERE execution_id = ? AND schema_version = ?
             ORDER BY phase_number ASC",
        )
        .bind(execution_id)
        .bind(THESIS_MEMORY_SCHEMA_VERSION)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("failed to load full report for execution_id={execution_id}"))
        .map_err(TradingError::Storage)?;

        let mut snapshots = Vec::with_capacity(rows.len());
        let mut skipped_phases = Vec::new();

        for (phase_number, state_json, usage_json) in rows {
            let state: TradingState = match serde_json::from_str(&state_json) {
                Ok(s) => s,
                Err(_) => {
                    warn!(
                        execution_id,
                        phase_number,
                        error.kind = "deserialize",
                        "report snapshot failed to deserialize; skipping"
                    );
                    skipped_phases.push(phase_number);
                    continue;
                }
            };

            let token_usage = usage_json.and_then(|json| {
                match serde_json::from_str::<Vec<AgentTokenUsage>>(&json) {
                    Ok(u) => Some(u),
                    Err(_) => {
                        warn!(
                            execution_id,
                            phase_number,
                            error.kind = "deserialize",
                            "report token usage failed to deserialize; degrading to None"
                        );
                        None
                    }
                }
            });

            snapshots.push(LoadedReportSnapshot {
                state,
                token_usage,
                phase_number,
            });
        }

        Ok(LoadedReport {
            snapshots,
            skipped_phases,
        })
    }
}

fn parse_snapshot_timestamp(raw: &str) -> anyhow::Result<chrono::DateTime<Utc>> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(parsed.with_timezone(&Utc));
    }

    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(raw, format) {
            return Ok(parsed.and_utc());
        }
    }

    Err(anyhow::anyhow!("unrecognized snapshot timestamp: {raw}"))
}
