use anyhow::Context as _;
use chrono::Utc;
use tracing::{debug, warn};

use crate::{
    error::TradingError,
    state::{ThesisMemory, TradingState},
};

use super::SnapshotStore;

const THESIS_MEMORY_SCHEMA_VERSION: i64 = 1;

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
            if schema_version > THESIS_MEMORY_SCHEMA_VERSION {
                debug!(
                    symbol,
                    schema_version,
                    "prior-thesis snapshot uses unsupported schema version; skipping"
                );
                continue;
            }

            let state: TradingState = match serde_json::from_str(&state_json) {
                Ok(s) => s,
                Err(err) => {
                    warn!(
                        symbol,
                        schema_version,
                        %err,
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
}
