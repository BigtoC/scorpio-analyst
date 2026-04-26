//! Activation-path audit (Unit 4a Step 3 deliverable).
//!
//! Asserts that every reachable `TradingPipeline` construction path produces
//! a graph whose entry task is `PreflightTask`. The audit is the structural
//! companion to the runtime contract: `PreflightTask` is the sole writer of
//! `state.analysis_runtime_policy`, the sole runner of
//! `validate_active_pack_completeness`, and the sole writer of
//! `KEY_RUNTIME_POLICY` / `KEY_ROUTING_FLAGS` to context. If any production
//! construction path bypassed preflight, downstream tasks would observe
//! missing context keys and the completeness gate would never fire.
//!
//! Exempt paths:
//! - `replace_task_for_test` and `__from_parts` are documented test-only
//!   seams that may swap individual tasks; they are out of scope here.
//! - The `install_stub_tasks_for_test` path replaces analyst/researcher/
//!   trader/risk/fund-manager tasks but keeps `PreflightTask` intact, which
//!   is what this audit pins.

#![cfg(feature = "test-helpers")]

use std::sync::Arc;

use scorpio_core::analysis_packs::{PackId, resolve_pack};
use scorpio_core::config::{Config, LlmConfig, TradingConfig};
use scorpio_core::data::{FinnhubClient, FredClient, YFinanceClient};
use scorpio_core::providers::factory::CompletionModelHandle;
use scorpio_core::rate_limit::SharedRateLimiter;
use scorpio_core::workflow::{PipelineDeps, SnapshotStore, TradingPipeline, build_graph_from_pack};

const PREFLIGHT_TASK_ID: &str = "preflight";

fn baseline_config(analysis_pack: &str) -> Config {
    Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: TradingConfig::default(),
        api: Default::default(),
        providers: Default::default(),
        storage: Default::default(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
        analysis_pack: analysis_pack.to_owned(),
    }
}

async fn make_test_snapshot_store(name: &str) -> (SnapshotStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    let store = SnapshotStore::new(Some(path.as_path()))
        .await
        .expect("snapshot store");
    (store, dir)
}

fn dummy_yfinance() -> YFinanceClient {
    YFinanceClient::new(SharedRateLimiter::new("activation-audit", 10))
}

#[tokio::test]
async fn pipeline_new_entry_task_is_preflight() {
    let config = baseline_config("baseline");
    let (snapshot_store, _dir) = make_test_snapshot_store("audit-new.db").await;
    let pipeline = TradingPipeline::new(
        config,
        FinnhubClient::for_test(),
        FredClient::for_test(),
        dummy_yfinance(),
        snapshot_store,
        CompletionModelHandle::for_test(),
        CompletionModelHandle::for_test(),
    );
    let graph = pipeline.build_graph();
    assert_eq!(
        graph.start_task_id().as_deref(),
        Some(PREFLIGHT_TASK_ID),
        "TradingPipeline::new must produce a graph whose entry task is PreflightTask"
    );
}

#[tokio::test]
async fn pipeline_try_new_entry_task_is_preflight() {
    let config = baseline_config("baseline");
    let (snapshot_store, _dir) = make_test_snapshot_store("audit-try-new.db").await;
    let pipeline = TradingPipeline::try_new(
        config,
        FinnhubClient::for_test(),
        FredClient::for_test(),
        dummy_yfinance(),
        snapshot_store,
        CompletionModelHandle::for_test(),
        CompletionModelHandle::for_test(),
    )
    .expect("baseline pack must resolve");
    let graph = pipeline.build_graph();
    assert_eq!(
        graph.start_task_id().as_deref(),
        Some(PREFLIGHT_TASK_ID),
        "TradingPipeline::try_new must produce a graph whose entry task is PreflightTask"
    );
}

#[tokio::test]
async fn pipeline_from_pack_entry_task_is_preflight() {
    let pack = resolve_pack(PackId::Baseline);
    let config = baseline_config("baseline");
    let (snapshot_store, _dir) = make_test_snapshot_store("audit-from-pack.db").await;
    let deps = PipelineDeps {
        config,
        finnhub: FinnhubClient::for_test(),
        fred: FredClient::for_test(),
        yfinance: dummy_yfinance(),
        snapshot_store,
        quick_handle: CompletionModelHandle::for_test(),
        deep_handle: CompletionModelHandle::for_test(),
    };
    let pipeline = TradingPipeline::from_pack(&pack, deps);
    let graph = pipeline.build_graph();
    assert_eq!(
        graph.start_task_id().as_deref(),
        Some(PREFLIGHT_TASK_ID),
        "TradingPipeline::from_pack must produce a graph whose entry task is PreflightTask"
    );
}

#[tokio::test]
async fn build_graph_from_pack_entry_task_is_preflight() {
    // Direct `build_graph_from_pack` callers (test fixtures, future
    // feature-gated entries) must also start at preflight — this pins the
    // contract at the lowest construction layer.
    let pack = resolve_pack(PackId::Baseline);
    let config = Arc::new(baseline_config("baseline"));
    let (snapshot_store, _dir) = make_test_snapshot_store("audit-build-graph.db").await;
    let snapshot_store = Arc::new(snapshot_store);
    let registry = scorpio_core::agents::analyst::AnalystRegistry::all_known();
    let graph = build_graph_from_pack(
        &pack,
        Arc::clone(&config),
        &registry,
        &FinnhubClient::for_test(),
        &FredClient::for_test(),
        &dummy_yfinance(),
        Arc::clone(&snapshot_store),
        &CompletionModelHandle::for_test(),
        &CompletionModelHandle::for_test(),
    );
    assert_eq!(
        graph.start_task_id().as_deref(),
        Some(PREFLIGHT_TASK_ID),
        "build_graph_from_pack must produce a graph whose entry task is PreflightTask"
    );
}
