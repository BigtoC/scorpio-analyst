use anyhow::Context as _;
use chrono::Utc;
use tracing::{debug, warn};

use crate::{
    error::TradingError,
    state::{ThesisMemory, TradingState},
};

use super::SnapshotStore;

/// Active thesis-memory schema version.
///
/// # v2 (Phase 6 reshape)
///
/// `TradingState` moved equity-only fields (`fundamental_metrics`,
/// `evidence_*`, `market_volatility`, `derived_valuation`, …) off the root
/// and into a new `equity: Option<EquityState>` sub-state. v1 snapshots
/// have those fields at the root, so they cannot be deserialized under the
/// new shape — the lookup below skips any row whose `schema_version` does
/// not equal the active version.
///
/// # Release note
///
/// Bumping this version is a one-time breaking change: existing
/// thesis-memory continuity is reset; prior-run theses will not be carried
/// forward. No SQL migration runs — pre-v3 rows remain on disk as
/// unsupported but are silently skipped on read in either direction (a v3
/// binary skips v2 rows; a v2 binary running against a database that already
/// contains v3 rows skips them via the same `!=` check). Developers may
/// optionally delete `~/.scorpio-analyst/phase_snapshots.db` for a clean
/// slate or run `DELETE FROM phase_snapshots WHERE schema_version < 3`.
pub(crate) const THESIS_MEMORY_SCHEMA_VERSION: i64 = 3;

impl SnapshotStore {
    /// Load the most recent prior thesis for a canonical symbol.
    ///
    /// Queries phase-5 snapshots for `symbol` that are no older than
    /// `max_age_days`. Returns the `current_thesis` field from the most recent
    /// matching snapshot's `TradingState`, or `None` if no compatible snapshot
    /// exists.
    ///
    /// Legacy rows that predate thesis-memory metadata (`symbol IS NULL`) are
    /// still considered by extracting `asset_symbol` from `trading_state_json`.
    /// Rows from unsupported schema versions are skipped as incompatible.
    /// Rows that fail deserialization due to schema evolution are skipped with a
    /// warning — struct changes between code versions are not treated as corruption.
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

        let rows: Vec<(Option<i64>, String)> = sqlx::query_as(
            "SELECT schema_version, trading_state_json
             FROM phase_snapshots
             WHERE phase_number = 5 AND created_at >= ?
               AND (
                    symbol = ?
                    OR (
                        symbol IS NULL
                        AND json_extract(trading_state_json, '$.asset_symbol') = ?
                    )
               )
             ORDER BY created_at DESC",
        )
        .bind(&cutoff)
        .bind(symbol)
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("failed to query prior-thesis snapshots for symbol={symbol}"))
        .map_err(TradingError::Storage)?;

        for (schema_version, state_json) in rows {
            let schema_version = schema_version.unwrap_or(0);
            // Same-version-only after the Phase 6 bump: incompatible rows are
            // skipped *before* deserialization so a v1 snapshot (which would
            // fail to decode into the new `equity`-shaped `TradingState`)
            // never surfaces as a fallback thesis.
            if schema_version != THESIS_MEMORY_SCHEMA_VERSION {
                debug!(
                    symbol,
                    schema_version,
                    active = THESIS_MEMORY_SCHEMA_VERSION,
                    "prior-thesis snapshot schema version mismatch; skipping"
                );
                continue;
            }

            let state: TradingState = match serde_json::from_str(&state_json) {
                Ok(s) => s,
                Err(_err) => {
                    // Drop `%err` from the log line: `serde_json` error
                    // formatting can echo offending payload bytes (snippets,
                    // type-error contexts) which would leak `trading_state_json`
                    // contents to log aggregators. Emit only a structural
                    // category tag so future log review never has to redact.
                    warn!(
                        symbol,
                        schema_version,
                        error.kind = "deserialize",
                        "prior-thesis snapshot failed to deserialize (schema evolution); skipping"
                    );
                    continue;
                }
            };

            if let Some(thesis) = state.current_thesis {
                return Ok(Some(thesis));
            }

            debug!(
                symbol,
                schema_version, "prior phase-5 snapshot has no current_thesis; skipping"
            );
        }

        Ok(None)
    }

    /// Load the most recent prior consensus-enrichment payload for a canonical symbol.
    ///
    /// Queries phase-1 snapshots for `symbol` that are no older than `max_age_days`
    /// and returns the newest compatible `enrichment_consensus.payload`, if any.
    ///
    /// Rows from unsupported schema versions are skipped. Rows that fail full
    /// `TradingState` deserialization are also skipped so stale snapshots never
    /// block a new run.
    pub async fn load_prior_consensus_for_symbol(
        &self,
        symbol: &str,
        max_age_days: i64,
    ) -> Result<Option<crate::data::adapters::estimates::ConsensusEvidence>, TradingError> {
        let cutoff = (Utc::now() - chrono::Duration::days(max_age_days)).to_rfc3339();

        let rows: Vec<(Option<i64>, String)> = sqlx::query_as(
            "SELECT schema_version, trading_state_json
             FROM phase_snapshots
             WHERE phase_number = 1 AND created_at >= ?
               AND (
                    symbol = ?
                    OR (
                        symbol IS NULL
                        AND json_extract(trading_state_json, '$.asset_symbol') = ?
                    )
               )
             ORDER BY created_at DESC",
        )
        .bind(&cutoff)
        .bind(symbol)
        .bind(symbol)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("failed to query prior consensus snapshots for symbol={symbol}"))
        .map_err(TradingError::Storage)?;

        for (schema_version, state_json) in rows {
            let schema_version = schema_version.unwrap_or(0);
            if schema_version != THESIS_MEMORY_SCHEMA_VERSION {
                debug!(
                    symbol,
                    schema_version,
                    active = THESIS_MEMORY_SCHEMA_VERSION,
                    "prior-consensus snapshot schema version mismatch; skipping"
                );
                continue;
            }

            let state: TradingState = match serde_json::from_str(&state_json) {
                Ok(s) => s,
                Err(_err) => {
                    warn!(
                        symbol,
                        schema_version,
                        error.kind = "deserialize",
                        "prior-consensus snapshot failed to deserialize (schema evolution); skipping"
                    );
                    continue;
                }
            };

            if let Some(consensus) = state.enrichment_consensus.payload {
                return Ok(Some(consensus));
            }

            debug!(
                symbol,
                schema_version, "prior phase-1 snapshot has no consensus payload; skipping"
            );
        }

        Ok(None)
    }
}
