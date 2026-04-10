//! Preflight task — the first node in the trading pipeline graph.
//!
//! [`PreflightTask`] runs before any analyst fan-out begins.  It:
//!
//! 1. Loads [`TradingState`] from the workflow context.
//! 2. Validates and canonicalises the runtime asset symbol using
//!    [`crate::data::resolve_symbol`].
//! 3. Writes the canonical symbol back into `TradingState.asset_symbol`.
//! 4. Loads the most recent compatible prior thesis from the snapshot store and
//!    attaches it to `TradingState.prior_thesis`.
//! 5. Re-serialises the updated state into the context.
//! 6. Writes all six Stage 1 evidence-provenance context keys.
//!
//! Any symbol format violation or context I/O failure causes the task to return
//! `Err`, halting the pipeline before a single analyst task is dispatched
//! ("fail closed" semantics).  Missing prior thesis is fail-open: the run
//! continues with `prior_thesis = None`.

use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::debug;

use crate::{
    data::{adapters::ProviderCapabilities, resolve_symbol},
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
    },
};

use super::common::{
    KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT, KEY_PROVIDER_CAPABILITIES,
    KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
};

const TASK_ID: &str = "preflight";

/// Staleness window for prior thesis lookup: snapshots older than this are
/// ignored even if they exist for the same symbol.
const THESIS_MEMORY_MAX_AGE_DAYS: i64 = 30;

/// The first pipeline node.
///
/// Constructed with a reference to the enrichment config so it can derive
/// [`ProviderCapabilities`] at runtime without reloading the full config, and
/// with the shared [`SnapshotStore`] so it can load prior thesis memory.
pub struct PreflightTask {
    enrichment: crate::config::DataEnrichmentConfig,
    snapshot_store: Arc<SnapshotStore>,
}

impl PreflightTask {
    /// Create a new `PreflightTask` using the checked-in enrichment config.
    pub fn new(
        enrichment: crate::config::DataEnrichmentConfig,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Self {
        Self {
            enrichment,
            snapshot_store,
        }
    }
}

#[async_trait]
impl Task for PreflightTask {
    fn id(&self) -> &str {
        TASK_ID
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // ── Load state ────────────────────────────────────────────────────
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "PreflightTask: orchestration corruption: state deserialization failed: {e}"
                ))
            })?;

        // ── Resolve and canonicalise symbol ───────────────────────────────
        let instrument = resolve_symbol(&state.asset_symbol).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: invalid asset symbol {:?}: {e}",
                state.asset_symbol
            ))
        })?;

        // Write the canonical symbol back into TradingState.
        state.asset_symbol = instrument.canonical_symbol.clone();

        // ── Load prior thesis memory (fail-open) ──────────────────────────
        let prior_thesis = self
            .snapshot_store
            .load_prior_thesis_for_symbol(&state.asset_symbol, THESIS_MEMORY_MAX_AGE_DAYS)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "PreflightTask: thesis memory lookup failed: {e}"
                ))
            })?;

        if prior_thesis.is_some() {
            debug!(symbol = %state.asset_symbol, "loaded prior thesis memory");
        } else {
            debug!(
                symbol = %state.asset_symbol,
                "no prior thesis memory available for this symbol"
            );
        }
        state.prior_thesis = prior_thesis;

        // ── Re-serialise the updated state ────────────────────────────────
        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "PreflightTask: orchestration corruption: state serialization failed: {e}"
                ))
            })?;

        // ── Derive and write ProviderCapabilities ─────────────────────────
        let capabilities = ProviderCapabilities::from_config(&self.enrichment);
        let caps_json = serde_json::to_string(&capabilities).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: orchestration corruption: ProviderCapabilities serialization failed: {e}"
            ))
        })?;
        context.set(KEY_PROVIDER_CAPABILITIES, caps_json).await;

        // ── Write ResolvedInstrument ──────────────────────────────────────
        let instrument_json = serde_json::to_string(&instrument).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: orchestration corruption: ResolvedInstrument serialization failed: {e}"
            ))
        })?;
        context.set(KEY_RESOLVED_INSTRUMENT, instrument_json).await;

        // ── Write required coverage inputs (fixed ordered list) ───────────
        let required_inputs: Vec<&str> = vec!["fundamentals", "sentiment", "news", "technical"];
        let inputs_json = serde_json::to_string(&required_inputs).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: orchestration corruption: required_coverage_inputs serialization failed: {e}"
            ))
        })?;
        context.set(KEY_REQUIRED_COVERAGE_INPUTS, inputs_json).await;

        // ── Seed typed null placeholders for cache keys ───────────────────
        // Always written unconditionally so downstream consumers can `expect`
        // the key to be present and treat its absence as a programming error.
        context.set(KEY_CACHED_TRANSCRIPT, "null".to_owned()).await;
        context.set(KEY_CACHED_CONSENSUS, "null".to_owned()).await;
        context.set(KEY_CACHED_EVENT_FEED, "null".to_owned()).await;

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

#[cfg(test)]
mod tests;
