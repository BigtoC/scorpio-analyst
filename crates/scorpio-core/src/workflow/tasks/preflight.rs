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
//! 5. Re-serializes the updated state into the context.
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
    analysis_packs::{RuntimePolicy, validate_active_pack_completeness},
    data::{adapters::ProviderCapabilities, resolve_symbol},
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        topology::{RoutingFlags, build_run_topology},
    },
};

use super::common::{
    KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT, KEY_MAX_DEBATE_ROUNDS,
    KEY_MAX_RISK_ROUNDS, KEY_PROVIDER_CAPABILITIES, KEY_REQUIRED_COVERAGE_INPUTS,
    KEY_RESOLVED_INSTRUMENT, KEY_ROUTING_FLAGS, KEY_RUNTIME_POLICY,
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
    /// The resolved runtime policy or the deferred resolution error for the
    /// config-selected pack.
    runtime_policy: Result<RuntimePolicy, String>,
}

impl PreflightTask {
    /// Create a new `PreflightTask` with baseline pack defaults.
    ///
    /// Convenience constructor for tests; production code should use
    /// [`Self::with_pack`] to propagate the config-selected pack.
    #[cfg(any(test, feature = "test-helpers"))]
    #[allow(dead_code)]
    pub fn new(
        enrichment: crate::config::DataEnrichmentConfig,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Self {
        Self::with_runtime_policy(
            enrichment,
            snapshot_store,
            crate::analysis_packs::resolve_runtime_policy("baseline")
                .expect("baseline pack must resolve"),
        )
    }

    /// Create a new `PreflightTask` with a specific pack selection.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_pack(
        enrichment: crate::config::DataEnrichmentConfig,
        snapshot_store: Arc<SnapshotStore>,
        pack_id: String,
    ) -> Self {
        Self {
            enrichment,
            snapshot_store,
            runtime_policy: crate::analysis_packs::resolve_runtime_policy(&pack_id),
        }
    }

    /// Create a new `PreflightTask` from an already-resolved runtime policy.
    pub fn with_runtime_policy(
        enrichment: crate::config::DataEnrichmentConfig,
        snapshot_store: Arc<SnapshotStore>,
        runtime_policy: RuntimePolicy,
    ) -> Self {
        Self {
            enrichment,
            snapshot_store,
            runtime_policy: Ok(runtime_policy),
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

        // Write the canonical symbol back into TradingState via the typed
        // setter so `asset_symbol` and `symbol` cannot drift.
        state.set_symbol(instrument.symbol.clone());

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

        // ── Resolve analysis pack into runtime policy ─────────────────────
        let runtime_policy = self.runtime_policy.as_ref().map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: pack resolution failed: {e}"
            ))
        })?;
        debug!(pack = %runtime_policy.pack_id, "resolved analysis pack");

        state.analysis_pack_name = Some(runtime_policy.pack_id.to_string());
        state.analysis_runtime_policy = Some(runtime_policy.clone());

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "PreflightTask: orchestration corruption: state serialization failed after runtime policy hydration: {e}"
                ))
            })?;

        // ── Write required coverage inputs from runtime policy ────────────
        let inputs_json = serde_json::to_string(&runtime_policy.required_inputs).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: orchestration corruption: required_coverage_inputs serialization failed: {e}"
            ))
        })?;
        context.set(KEY_REQUIRED_COVERAGE_INPUTS, inputs_json).await;

        let policy_json = serde_json::to_string(runtime_policy).map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: orchestration corruption: RuntimePolicy serialization failed: {e}"
            ))
        })?;
        context.set(KEY_RUNTIME_POLICY, policy_json).await;

        // ── Build per-run topology + routing flags ───────────────────────
        // Read the configured round counts already written into context by
        // `run_analysis_cycle`. Falling back to zero on a missing key is
        // structurally safe because the conditional-edge closures already
        // tolerate `None` the same way; in practice both keys are present
        // for every active pipeline run.
        let max_debate_rounds = context.get_sync::<u32>(KEY_MAX_DEBATE_ROUNDS).unwrap_or(0);
        let max_risk_rounds = context.get_sync::<u32>(KEY_MAX_RISK_ROUNDS).unwrap_or(0);
        let topology = build_run_topology(
            &runtime_policy.required_inputs,
            max_debate_rounds,
            max_risk_rounds,
        );
        let routing_flags = RoutingFlags::from_topology(&topology);
        // Store RoutingFlags as a typed struct so the conditional-edge
        // closures in `workflow::builder` can read it via `get_sync` without
        // a JSON round-trip on every iteration.
        context.set(KEY_ROUTING_FLAGS, routing_flags).await;

        // ── Active-pack completeness gate (fail-loud) ─────────────────────
        // Validate the resolved runtime policy that this graph will actually
        // use against the topology. Failures surface as
        // `TaskExecutionFailed`, halting the pipeline before any analyst or
        // model task fires. The baseline pack is complete under the fully
        // enabled topology (covered by `analysis_packs::completeness::tests`
        // and the regression-gate sanity), so production runs never trip
        // this gate today; it exists to defend against future packs that
        // ship incomplete prompt bundles.
        if let Err(err) = validate_active_pack_completeness(runtime_policy, &topology) {
            return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: active pack {:?} is incomplete under runtime topology: {err}",
                err.pack_id
            )));
        }

        // ── Pack-author injection guard (defense-in-depth) ────────────────
        // `analysis_emphasis` is pack-manifest-owned and compile-time embedded
        // for builtin packs, but a future runtime-loaded pack or a careless
        // edit could ship adversarial content. The strict 0x20-0x7E ASCII +
        // role-injection-tag rejection runs here so a malformed value fails
        // before substitution into any LLM system prompt. Builtin packs
        // produce well-formed values today, so this gate is silent for
        // production runs.
        if let Err(err) =
            crate::prompts::sanitize_analysis_emphasis(&runtime_policy.analysis_emphasis)
        {
            return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                "PreflightTask: pack {:?} has invalid analysis_emphasis: {err}",
                runtime_policy.pack_id
            )));
        }

        // ── Seed typed null placeholders for cache keys ───────────────────
        // Each key is always present after preflight so downstream consumers can
        // `expect` it.  However, if `run_analysis_cycle` has already hydrated a
        // key with real enrichment data (non-null JSON), we preserve it instead
        // of overwriting with `"null"`.
        seed_if_absent(&context, KEY_CACHED_TRANSCRIPT).await;
        seed_if_absent(&context, KEY_CACHED_CONSENSUS).await;
        seed_if_absent(&context, KEY_CACHED_EVENT_FEED).await;

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Write `"null"` to a context key only if it is not already set or its current
/// value is the JSON literal `"null"`.  This preserves enrichment data that
/// `run_analysis_cycle` may have hydrated before the graph starts.
async fn seed_if_absent(context: &Context, key: &str) {
    let existing: Option<String> = context.get(key).await;
    match existing.as_deref() {
        None | Some("null") => {
            context.set(key, "null".to_owned()).await;
        }
        Some(_) => {
            // Already populated with real data — do not overwrite.
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use graph_flow::{Context, Task};

    use crate::{
        config::DataEnrichmentConfig,
        data::{ResolvedInstrument, adapters::ProviderCapabilities},
        state::TradingState,
        workflow::{
            SnapshotStore,
            context_bridge::{deserialize_state_from_context, serialize_state_to_context},
            snapshot::SnapshotPhase,
            tasks::common::{
                KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_TRANSCRIPT,
                KEY_PROVIDER_CAPABILITIES, KEY_REQUIRED_COVERAGE_INPUTS, KEY_RESOLVED_INSTRUMENT,
            },
        },
    };

    use super::PreflightTask;
    use crate::workflow::tasks::common::KEY_RUNTIME_POLICY;

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Open a temporary on-disk snapshot store for preflight tests.
    async fn test_store() -> (Arc<SnapshotStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("preflight-test.db");
        let store = SnapshotStore::new(Some(&path))
            .await
            .expect("store should open");
        (Arc::new(store), dir)
    }

    async fn run_preflight(
        symbol: &str,
        enrichment: DataEnrichmentConfig,
    ) -> graph_flow::Result<Context> {
        let (store, _dir) = test_store().await;
        run_preflight_with_store(symbol, enrichment, store).await
    }

    async fn run_preflight_with_store(
        symbol: &str,
        enrichment: DataEnrichmentConfig,
        store: Arc<SnapshotStore>,
    ) -> graph_flow::Result<Context> {
        let state = TradingState::new(symbol, "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let task = PreflightTask::new(enrichment, store);
        task.run(ctx.clone()).await?;
        Ok(ctx)
    }

    // ── Basic / fail-closed tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn preflight_fails_closed_on_invalid_symbol() {
        let state = TradingState::new("DROP;TABLE", "2026-01-15");
        let (store, _dir) = test_store().await;
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
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
        let (store, _dir) = test_store().await;
        let ctx = Context::new();
        let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
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

    // ── Context-contract tests ────────────────────────────────────────────────

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
            ..DataEnrichmentConfig::default()
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

    // ── Enrichment hydration preservation tests ────────────────────────────

    #[tokio::test]
    async fn preflight_preserves_pre_hydrated_consensus_data() {
        let (store, _dir) = test_store().await;
        let state = TradingState::new("AAPL", "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        // Pre-hydrate consensus cache key with real data.
        let real_data = r#"{"symbol":"AAPL","eps_estimate":2.5,"revenue_estimate_m":95000.0,"analyst_count":35,"as_of_date":"2026-01-15"}"#;
        ctx.set(KEY_CACHED_CONSENSUS, real_data.to_owned()).await;

        let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
        task.run(ctx.clone())
            .await
            .expect("preflight should succeed");

        let after: String = ctx.get(KEY_CACHED_CONSENSUS).await.expect("key must exist");
        assert_eq!(
            after, real_data,
            "preflight must not overwrite pre-hydrated enrichment data"
        );
    }

    #[tokio::test]
    async fn preflight_preserves_pre_hydrated_event_feed_data() {
        let (store, _dir) = test_store().await;
        let state = TradingState::new("AAPL", "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let real_data = r#"[{"symbol":"AAPL","event_timestamp":"2026-01-14T18:00:00Z","event_type":"earnings_release","headline":"Apple beats Q1","impact":"positive"}]"#;
        ctx.set(KEY_CACHED_EVENT_FEED, real_data.to_owned()).await;

        let task = PreflightTask::new(DataEnrichmentConfig::default(), store);
        task.run(ctx.clone())
            .await
            .expect("preflight should succeed");

        let after: String = ctx
            .get(KEY_CACHED_EVENT_FEED)
            .await
            .expect("key must exist");
        assert_eq!(
            after, real_data,
            "preflight must not overwrite pre-hydrated event feed data"
        );
    }

    #[tokio::test]
    async fn preflight_seeds_null_when_no_enrichment_pre_hydrated() {
        let ctx = run_preflight("MSFT", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        // Without pre-hydration, all enrichment keys should be "null".
        for key in [
            KEY_CACHED_TRANSCRIPT,
            KEY_CACHED_CONSENSUS,
            KEY_CACHED_EVENT_FEED,
        ] {
            let raw: String = ctx.get(key).await.expect("key must be present");
            assert_eq!(
                raw, "null",
                "key '{key}' should be null without pre-hydration"
            );
        }
    }

    // ── Thesis-memory tests ───────────────────────────────────────────────────

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

    // ── Runtime policy context key tests ─────────────────────────────────────

    #[tokio::test]
    async fn preflight_writes_runtime_policy_to_context() {
        let ctx = run_preflight("AAPL", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let json: String = ctx
            .get(KEY_RUNTIME_POLICY)
            .await
            .expect("runtime_policy key must be present after preflight");
        let policy: crate::analysis_packs::RuntimePolicy =
            serde_json::from_str(&json).expect("RuntimePolicy deserialization");
        assert_eq!(
            policy.pack_id,
            crate::analysis_packs::PackId::Baseline,
            "default preflight should resolve baseline pack"
        );
    }

    #[tokio::test]
    async fn preflight_runtime_policy_has_baseline_required_inputs() {
        let ctx = run_preflight("MSFT", DataEnrichmentConfig::default())
            .await
            .expect("preflight should succeed");

        let json: String = ctx.get(KEY_RUNTIME_POLICY).await.unwrap();
        let policy: crate::analysis_packs::RuntimePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(
            policy.required_inputs,
            vec!["fundamentals", "sentiment", "news", "technical"]
        );
    }

    #[tokio::test]
    async fn preflight_with_invalid_pack_fails_before_analysis() {
        let (store, _dir) = test_store().await;
        let state = TradingState::new("AAPL", "2026-01-15");
        let ctx = Context::new();
        serialize_state_to_context(&state, &ctx)
            .await
            .expect("state serialization");

        let task = PreflightTask::with_pack(
            DataEnrichmentConfig::default(),
            store,
            "nonexistent_pack".to_owned(),
        );
        let result = task.run(ctx).await;
        assert!(
            result.is_err(),
            "invalid pack must fail before analysis starts (R6)"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("pack resolution failed"),
            "error should mention pack resolution: {msg}"
        );
    }

    #[tokio::test]
    async fn preflight_all_seven_context_keys_present_after_run() {
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
            KEY_RUNTIME_POLICY,
        ] {
            let val: Option<String> = ctx.get(key).await;
            assert!(
                val.is_some(),
                "context key '{key}' must be present after preflight"
            );
        }
    }
}
