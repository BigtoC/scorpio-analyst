//! Preflight task — the first node in the trading pipeline graph.
//!
//! [`PreflightTask`] runs before any analyst fan-out begins.  It:
//!
//! 1. Loads [`TradingState`] from the workflow context.
//! 2. Validates and canonicalises the runtime asset symbol using
//!    [`crate::data::resolve_symbol`].
//! 3. Writes the canonical symbol back into `TradingState.asset_symbol`.
//! 4. Re-serialises the updated state into the context.
//! 5. Writes all six Stage 1 evidence-provenance context keys.
//!
//! Any symbol format violation or context I/O failure causes the task to return
//! `Err`, halting the pipeline before a single analyst task is dispatched
//! ("fail closed" semantics).

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};

use crate::{
    data::{adapters::ProviderCapabilities, resolve_symbol},
    workflow::context_bridge::{deserialize_state_from_context, serialize_state_to_context},
};

use super::common::{
    KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT, KEY_PROVIDER_CAPABILITIES,
    KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
};

const TASK_ID: &str = "preflight";

/// The first pipeline node.
///
/// Constructed with a reference to the enrichment config so it can derive
/// [`ProviderCapabilities`] at runtime without reloading the full config.
pub struct PreflightTask {
    enrichment: crate::config::DataEnrichmentConfig,
}

impl PreflightTask {
    /// Create a new `PreflightTask` using the checked-in enrichment config.
    pub fn new(enrichment: crate::config::DataEnrichmentConfig) -> Self {
        Self { enrichment }
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
mod tests {
    use graph_flow::Context;

    use crate::{
        config::DataEnrichmentConfig,
        data::{ResolvedInstrument, adapters::ProviderCapabilities},
        state::TradingState,
        workflow::{
            context_bridge::{deserialize_state_from_context, serialize_state_to_context},
            tasks::common::{
                KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT,
                KEY_PROVIDER_CAPABILITIES, KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
            },
        },
    };

    use super::PreflightTask;
    use graph_flow::Task;

    async fn run_preflight(
        symbol: &str,
        enrichment: DataEnrichmentConfig,
    ) -> graph_flow::Result<Context> {
        let state = TradingState::new(symbol, "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let task = PreflightTask::new(enrichment);
        task.run(ctx.clone()).await?;
        Ok(ctx)
    }

    #[tokio::test]
    async fn preflight_writes_canonical_uppercase_symbol_to_state() {
        let ctx = run_preflight("nvda", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed with lowercase symbol");

        let state = deserialize_state_from_context(&ctx)
            .await
            .expect("state deserialization");
        assert_eq!(state.asset_symbol, "NVDA");
    }

    #[tokio::test]
    async fn preflight_writes_resolved_instrument_to_context() {
        let ctx = run_preflight("AAPL", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let json: String = ctx
            .get(KEY_RESOLVED_INSTRUMENT)
            .await
            .expect("resolved_instrument key must be present");
        let instrument: ResolvedInstrument =
            serde_json::from_str(&json).expect("ResolvedInstrument deserialization");
        assert_eq!(instrument.canonical_symbol, "AAPL");
    }

    #[tokio::test]
    async fn preflight_writes_provider_capabilities_to_context() {
        let enrichment = DataEnrichmentConfig {
            enable_transcripts: true,
            enable_consensus_estimates: false,
            enable_event_news: false,
            max_evidence_age_hours: 48,
        };
        let ctx = run_preflight("AAPL", enrichment)
            .await
            .expect("preflight should succeed");

        let json: String = ctx
            .get(KEY_PROVIDER_CAPABILITIES)
            .await
            .expect("provider_capabilities key must be present");
        let caps: ProviderCapabilities =
            serde_json::from_str(&json).expect("ProviderCapabilities deserialization");
        assert!(caps.transcripts);
        assert!(!caps.consensus_estimates);
        assert!(!caps.event_news);
    }

    #[tokio::test]
    async fn preflight_writes_required_coverage_inputs_in_fixed_order() {
        let ctx = run_preflight("MSFT", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let json: String = ctx
            .get(KEY_REQUIRED_COVERAGE_INPUTS)
            .await
            .expect("required_coverage_inputs key must be present");
        let inputs: Vec<String> =
            serde_json::from_str(&json).expect("required_coverage_inputs deserialization");
        assert_eq!(
            inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }

    #[tokio::test]
    async fn preflight_seeds_cached_transcript_as_null_placeholder() {
        let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let raw: String = ctx
            .get(KEY_CACHED_TRANSCRIPT)
            .await
            .expect("cached_transcript must be present");
        assert_eq!(raw, "null", "Stage 1 value must be the JSON literal 'null'");
    }

    #[tokio::test]
    async fn preflight_seeds_cached_consensus_as_null_placeholder() {
        let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let raw: String = ctx
            .get(KEY_CACHED_CONSENSUS)
            .await
            .expect("cached_consensus must be present");
        assert_eq!(raw, "null");
    }

    #[tokio::test]
    async fn preflight_seeds_cached_event_feed_as_null_placeholder() {
        let ctx = run_preflight("TSLA", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let raw: String = ctx
            .get(KEY_CACHED_EVENT_FEED)
            .await
            .expect("cached_event_feed must be present");
        assert_eq!(raw, "null");
    }

    #[tokio::test]
    async fn preflight_fails_closed_on_invalid_symbol() {
        let state = TradingState::new("DROP;TABLE", "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let task = PreflightTask::new(DataEnrichmentConfig::default());
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
        // Context has no trading_state key — simulates context corruption.
        let ctx = Context::new();
        let task = PreflightTask::new(DataEnrichmentConfig::default());
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

    #[tokio::test]
    async fn preflight_all_six_context_keys_present_after_run() {
        let ctx = run_preflight("BRK.B", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        for key in [
            KEY_RESOLVED_INSTRUMENT,
            KEY_PROVIDER_CAPABILITIES,
            KEY_REQUIRED_COVERAGE_INPUTS,
            KEY_CACHED_TRANSCRIPT,
            KEY_CACHED_CONSENSUS,
            KEY_CACHED_EVENT_FEED,
        ] {
            let val: Option<String> = ctx.get(key).await;
            assert!(
                val.is_some(),
                "context key '{key}' must be present after preflight"
            );
        }
    }
}
