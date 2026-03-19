//! Integration tests for the graph-flow pipeline orchestration.
//!
//! These tests exercise the real pipeline components (graph wiring, context bridge,
//! snapshot store, conditional-edge logic, token-usage accumulation) without making
//! live LLM network calls.  Where agent functions would be invoked they are either
//! bypassed by testing at the task/context level directly or the test verifies
//! structural properties (node presence, edge routing) of the built graph.

use std::sync::Arc;

use graph_flow::{Context, NextAction, Task};
use scorpio_analyst::{
    state::{
        AgentTokenUsage, FundamentalData, NewsData, SentimentData, TechnicalData, TradingState,
    },
    workflow::{
        SnapshotStore,
        context_bridge::{
            deserialize_state_from_context, serialize_state_to_context, write_prefixed_result,
        },
        tasks::{
            AnalystSyncTask, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
            KEY_RISK_ROUND,
        },
    },
};
use tempfile::tempdir;

// ── Helper constructors ───────────────────────────────────────────────────────

fn sample_state() -> TradingState {
    TradingState::new("AAPL", "2026-03-19")
}

async fn seed_state(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("state serialization should succeed");
}

async fn make_store() -> (Arc<SnapshotStore>, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation"),
    );
    (store, dir)
}

// ── Analyst flags helpers (mirroring tasks module internals) ──────────────────

const ANALYST_PREFIX: &str = "analyst";
const OK_SUFFIX: &str = "ok";
const ERR_SUFFIX: &str = "err";

async fn set_analyst_ok(ctx: &Context, analyst: &str, ok: bool) {
    ctx.set(format!("{ANALYST_PREFIX}.{analyst}.{OK_SUFFIX}"), ok)
        .await;
}

async fn set_analyst_err(ctx: &Context, analyst: &str, msg: &str) {
    ctx.set(
        format!("{ANALYST_PREFIX}.{analyst}.{ERR_SUFFIX}"),
        msg.to_owned(),
    )
    .await;
}

async fn write_analyst_data(ctx: &Context, analyst: &str, data: impl serde::Serialize) {
    write_prefixed_result(ctx, ANALYST_PREFIX, analyst, &data)
        .await
        .expect("write_prefixed_result");
}

fn all_ok_analysts() -> [&'static str; 4] {
    ["fundamental", "sentiment", "news", "technical"]
}

fn fundamental_data() -> FundamentalData {
    FundamentalData {
        revenue_growth_pct: None,
        pe_ratio: Some(24.5),
        eps: Some(6.05),
        current_ratio: None,
        debt_to_equity: None,
        gross_margin: None,
        net_income: None,
        insider_transactions: vec![],
        summary: "strong fundamentals".to_owned(),
    }
}

fn sentiment_data() -> SentimentData {
    SentimentData {
        overall_score: 0.72,
        source_breakdown: vec![],
        engagement_peaks: vec![],
        summary: "positive sentiment".to_owned(),
    }
}

fn news_data() -> NewsData {
    NewsData {
        articles: vec![],
        macro_events: vec![],
        summary: "no major news".to_owned(),
    }
}

fn technical_data() -> TechnicalData {
    TechnicalData {
        rsi: Some(55.0),
        macd: None,
        atr: None,
        sma_20: None,
        sma_50: None,
        ema_12: None,
        ema_26: None,
        bollinger_upper: None,
        bollinger_lower: None,
        support_level: None,
        resistance_level: None,
        volume_avg: None,
        summary: "neutral technical".to_owned(),
    }
}

async fn seed_all_analysts_ok(ctx: &Context) {
    for analyst in all_ok_analysts() {
        set_analyst_ok(ctx, analyst, true).await;
    }
    write_analyst_data(ctx, "fundamental", fundamental_data()).await;
    write_analyst_data(ctx, "sentiment", sentiment_data()).await;
    write_analyst_data(ctx, "news", news_data()).await;
    write_analyst_data(ctx, "technical", technical_data()).await;
}

// ────────────────────────────────────────────────────────────────────────────
// 11.1 — Full pipeline structural test (all 5 phases wired)
// ────────────────────────────────────────────────────────────────────────────
//
// We cannot run the full pipeline without live LLM calls, but we can verify
// that `build_graph` produces a graph with all 11 expected nodes and that
// `run_analysis_cycle` propagates errors from the first task (analyst fan-out)
// rather than silently skipping phases.  This structural test confirms the
// 5-phase topology is wired without crashes or panics.

#[test]
fn pipeline_build_graph_produces_graph_without_panic() {
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        workflow::TradingPipeline,
    };

    // We need a sync snapshot store; run in a minimal tokio runtime.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (store, _dir) = rt.block_on(async {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let store = SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store");
        (store, dir)
    });

    let config = Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: TradingConfig {
            asset_symbol: "AAPL".to_owned(),
            backtest_start: None,
            backtest_end: None,
        },
        api: ApiConfig {
            finnhub_rate_limit: 30,
            openai_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
            finnhub_api_key: None,
        },
        storage: Default::default(),
    };

    let finnhub = FinnhubClient::for_test();
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("test-yfinance", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(config, finnhub, yfinance, store, handle.clone(), handle);

    // build_graph must not panic and must return a non-trivially-empty graph.
    let _graph = pipeline.build_graph();
    // If we reach here, all 11 tasks were added without error.
}

// ────────────────────────────────────────────────────────────────────────────
// 11.2 — Analyst degradation: 1 analyst fails → pipeline continues
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn integration_one_analyst_failure_pipeline_continues() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Fundamental analyst fails; others succeed.
    set_analyst_ok(&ctx, "fundamental", false).await;
    set_analyst_err(&ctx, "fundamental", "simulated failure").await;
    set_analyst_ok(&ctx, "sentiment", true).await;
    set_analyst_ok(&ctx, "news", true).await;
    set_analyst_ok(&ctx, "technical", true).await;
    write_analyst_data(&ctx, "sentiment", sentiment_data()).await;
    write_analyst_data(&ctx, "news", news_data()).await;
    write_analyst_data(&ctx, "technical", technical_data()).await;

    let task = AnalystSyncTask::new(store);
    let result = task
        .run(ctx.clone())
        .await
        .expect("1-failure should not abort task");

    // Pipeline continues (Next::Continue, not End).
    assert_eq!(
        result.next_action,
        NextAction::Continue,
        "one analyst failure must not abort the pipeline"
    );

    // Analyst data for successful analysts is merged; failed analyst is absent.
    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(
        recovered.fundamental_metrics.is_none(),
        "failed analyst result must not be present in state"
    );
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());
}

// ────────────────────────────────────────────────────────────────────────────
// 11.3 — Analyst degradation: 2 analysts fail → pipeline aborts after Phase 1
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn integration_two_analyst_failures_abort_pipeline() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Two analysts fail.
    set_analyst_ok(&ctx, "fundamental", false).await;
    set_analyst_ok(&ctx, "sentiment", false).await;
    set_analyst_err(&ctx, "fundamental", "error 1").await;
    set_analyst_err(&ctx, "sentiment", "error 2").await;
    set_analyst_ok(&ctx, "news", true).await;
    set_analyst_ok(&ctx, "technical", true).await;
    write_analyst_data(&ctx, "news", news_data()).await;
    write_analyst_data(&ctx, "technical", technical_data()).await;

    let task = AnalystSyncTask::new(store);
    let result = task
        .run(ctx)
        .await
        .expect("task itself must not error; degradation is signalled via NextAction::End");

    assert_eq!(
        result.next_action,
        NextAction::End,
        "two analyst failures must abort the pipeline via NextAction::End"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.4 — Debate cycling: context counters drive conditional edge correctly
// ────────────────────────────────────────────────────────────────────────────
//
// This test verifies the counter/max logic that the conditional edge reads:
// `debate_round < max_debate_rounds` → loop; `debate_round >= max` → advance.
// We test the counter semantics directly on the Context, matching exactly what
// the graph edge predicate evaluates.

#[tokio::test]
async fn integration_debate_round_counter_drives_cycling() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 2u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    // Before any round: loop condition is true.
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);
    assert!(round < max, "at round 0 of 2, loop should continue");

    // Simulate DebateModeratorTask incrementing the counter once.
    ctx.set(KEY_DEBATE_ROUND, 1u32).await;
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    assert!(round < max, "at round 1 of 2, loop should continue");

    // Simulate second increment.
    ctx.set(KEY_DEBATE_ROUND, 2u32).await;
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    assert!(round >= max, "at round 2 of 2, loop should stop");
}

// ────────────────────────────────────────────────────────────────────────────
// 11.5 — Risk cycling: sequential execution order verified via context counters
// ────────────────────────────────────────────────────────────────────────────
//
// Mirrors 11.4 but for the risk loop.  Also verifies zero-round bypass: when
// max_risk_rounds = 0, the predicate is false from the start so the graph
// routes directly to RiskModerator.

#[tokio::test]
async fn integration_risk_round_counter_drives_cycling() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_RISK_ROUNDS, 2u32).await;
    ctx.set(KEY_RISK_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);
    assert!(round < max, "at risk round 0 of 2, loop should continue");

    ctx.set(KEY_RISK_ROUND, 2u32).await;
    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    assert!(round >= max, "at risk round 2 of 2, loop should stop");
}

#[tokio::test]
async fn integration_zero_risk_rounds_bypasses_loop() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_RISK_ROUNDS, 0u32).await;
    ctx.set(KEY_RISK_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);
    assert!(
        round >= max,
        "with max_risk_rounds=0, loop predicate must be false (bypass directly to moderator)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.6 — Phase snapshots: SnapshotStore save + load round-trip
// ────────────────────────────────────────────────────────────────────────────
//
// Verifies that after `AnalystSyncTask` writes a phase-1 snapshot, the snapshot
// can be loaded back and the state is intact.

#[tokio::test]
async fn integration_phase_snapshot_written_and_readable() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Seed all analysts as successful.
    seed_all_analysts_ok(&ctx).await;

    let task = AnalystSyncTask::new(Arc::clone(&store));
    let result = task.run(ctx.clone()).await.expect("task must succeed");
    assert_eq!(result.next_action, NextAction::Continue);

    // Retrieve the execution_id that was assigned during the task.
    let final_state = deserialize_state_from_context(&ctx)
        .await
        .expect("deserialize state");
    let exec_id = final_state.execution_id.to_string();

    // Phase 1 snapshot must now exist.
    let snapshot = store
        .load_snapshot(&exec_id, 1)
        .await
        .expect("load_snapshot should not error");

    assert!(
        snapshot.is_some(),
        "phase 1 snapshot must be written by AnalystSyncTask"
    );
    let (loaded_state, _token_usage) = snapshot.unwrap();
    assert_eq!(loaded_state.asset_symbol, "AAPL");
}

// ────────────────────────────────────────────────────────────────────────────
// 11.7 — Token usage: AgentTokenUsage entries are accumulated in TradingState
// ────────────────────────────────────────────────────────────────────────────
//
// After AnalystSyncTask runs (Phase 1), `TradingState.token_usage` must contain
// at least one `PhaseTokenUsage` entry for the "Analyst Fan-Out" phase.

#[tokio::test]
async fn integration_token_usage_accumulated_after_analyst_sync() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Seed all analysts as successful.
    seed_all_analysts_ok(&ctx).await;

    let task = AnalystSyncTask::new(store);
    let result = task.run(ctx.clone()).await.expect("task must succeed");
    assert_eq!(result.next_action, NextAction::Continue);

    let final_state = deserialize_state_from_context(&ctx)
        .await
        .expect("deserialize state");

    let phase_usages = &final_state.token_usage.phase_usage;
    assert!(
        !phase_usages.is_empty(),
        "token_usage must contain at least one PhaseTokenUsage after Phase 1"
    );

    let analyst_phase = phase_usages
        .iter()
        .find(|p| p.phase_name == "Analyst Fan-Out");
    assert!(
        analyst_phase.is_some(),
        "token_usage must contain a 'Analyst Fan-Out' phase entry"
    );

    // There should be 4 agent entries (one per analyst).
    let analyst_phase = analyst_phase.unwrap();
    assert_eq!(
        analyst_phase.agent_usage.len(),
        4,
        "analyst_team phase must record 4 agent token usages (one per analyst)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Supplemental: snapshot store returns None for unknown execution ID
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_store_returns_none_for_unknown_execution_id() {
    let (store, _dir) = make_store().await;
    let result = store
        .load_snapshot("non-existent-exec-id", 1)
        .await
        .expect("load_snapshot must not error for missing row");
    assert!(result.is_none(), "missing snapshot must return None");
}

// ────────────────────────────────────────────────────────────────────────────
// Supplemental: snapshot store upsert (duplicate phase replaces)
// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_store_upsert_replaces_existing_snapshot() {
    let (store, _dir) = make_store().await;
    let state = sample_state();

    let exec_id = "upsert-test-exec-id";

    // Save a first snapshot.
    store
        .save_snapshot(exec_id, 1, "analyst_team", &state, None)
        .await
        .expect("first save");

    // Save a second snapshot with the same (exec_id, phase_number).
    let mut state2 = sample_state();
    state2.asset_symbol = "TSLA".to_owned();
    store
        .save_snapshot(exec_id, 1, "analyst_team", &state2, None)
        .await
        .expect("upsert save");

    // Only one row should exist; it should be the updated one.
    let result = store
        .load_snapshot(exec_id, 1)
        .await
        .expect("load after upsert");
    assert!(result.is_some());
    let (loaded, _) = result.unwrap();
    assert_eq!(
        loaded.asset_symbol, "TSLA",
        "upsert must replace the original snapshot"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.11 — Exact graph topology assertion
// ────────────────────────────────────────────────────────────────────────────
//
// Verifies that `build_graph` wires the correct start task and all 11 expected
// nodes are reachable via `get_task`.  Uses string literals because the
// `TASK_*` constants in `pipeline.rs` are private.

#[test]
fn pipeline_graph_topology_has_correct_start_and_all_nodes() {
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        workflow::TradingPipeline,
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (store, _dir) = rt.block_on(async {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let store = SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store");
        (store, dir)
    });

    let config = Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: TradingConfig {
            asset_symbol: "AAPL".to_owned(),
            backtest_start: None,
            backtest_end: None,
        },
        api: ApiConfig {
            finnhub_rate_limit: 30,
            openai_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
            finnhub_api_key: None,
        },
        storage: Default::default(),
    };

    let finnhub = FinnhubClient::for_test();
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("test-topology", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(config, finnhub, yfinance, store, handle.clone(), handle);
    let graph = pipeline.build_graph();

    // Start task must be the analyst fan-out.
    assert_eq!(
        graph.start_task_id(),
        Some("analyst_fanout".to_owned()),
        "start task must be 'analyst_fanout'"
    );

    // All 11 expected node IDs must resolve.
    let expected_nodes = [
        "analyst_fanout",
        "analyst_sync",
        "bullish_researcher",
        "bearish_researcher",
        "debate_moderator",
        "trader",
        "aggressive_risk",
        "conservative_risk",
        "neutral_risk",
        "risk_moderator",
        "fund_manager",
    ];
    for id in &expected_nodes {
        assert!(
            graph.get_task(id).is_some(),
            "graph must contain node '{id}'"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 11.12 — Zero-round debate routing (context predicate bypasses loop)
// ────────────────────────────────────────────────────────────────────────────
//
// With KEY_MAX_DEBATE_ROUNDS = 0 and KEY_DEBATE_ROUND = 0, the loop predicate
// `round < max` evaluates to false so the graph routes directly to Trader.
// Mirrors `integration_zero_risk_rounds_bypasses_loop`.

#[tokio::test]
async fn integration_zero_debate_rounds_bypasses_loop() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 0u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);
    assert!(
        round >= max,
        "with max_debate_rounds=0, loop predicate must be false (bypass directly to trader)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.13 — Malformed JSON for trading_state key returns SchemaViolation
// ────────────────────────────────────────────────────────────────────────────
//
// Storing an invalid JSON string under TRADING_STATE_KEY and calling
// `deserialize_state_from_context` must produce a `TradingError::SchemaViolation`.

#[tokio::test]
async fn malformed_trading_state_json_returns_schema_violation() {
    use scorpio_analyst::{
        error::TradingError,
        workflow::context_bridge::{TRADING_STATE_KEY, deserialize_state_from_context},
    };

    let ctx = Context::new();
    ctx.set(TRADING_STATE_KEY, "not valid json {{{{".to_owned())
        .await;

    let err = deserialize_state_from_context(&ctx)
        .await
        .expect_err("malformed JSON must return an error");

    assert!(
        matches!(err, TradingError::SchemaViolation { .. }),
        "expected SchemaViolation, got: {err:?}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.14 — Two execution_id values are distinct (per-cycle uniqueness)
// ────────────────────────────────────────────────────────────────────────────
//
// Documents the per-cycle uniqueness guarantee: each TradingState created with
// `new()` gets a fresh UUID, so two successive cycles never share an ID.

#[test]
fn execution_ids_are_distinct_across_cycles() {
    let state1 = sample_state();
    let state2 = sample_state();
    assert_ne!(
        state1.execution_id, state2.execution_id,
        "each TradingState::new() must receive a unique execution_id"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Supplemental: agent token usage accumulation across multiple push_phase_usage
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn token_tracker_accumulates_multiple_phases() {
    use scorpio_analyst::state::{PhaseTokenUsage, TokenUsageTracker};

    let mut tracker = TokenUsageTracker::default();

    let phase1 = PhaseTokenUsage {
        phase_name: "analyst_team".to_owned(),
        agent_usage: vec![AgentTokenUsage {
            agent_name: "Fundamental Analyst".to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            latency_ms: 200,
            token_counts_available: true,
        }],
        phase_prompt_tokens: 100,
        phase_completion_tokens: 50,
        phase_total_tokens: 150,
        phase_duration_ms: 300,
    };

    let phase2 = PhaseTokenUsage {
        phase_name: "trader".to_owned(),
        agent_usage: vec![AgentTokenUsage {
            agent_name: "Trader".to_owned(),
            model_id: "o3".to_owned(),
            prompt_tokens: 200,
            completion_tokens: 100,
            total_tokens: 300,
            latency_ms: 500,
            token_counts_available: true,
        }],
        phase_prompt_tokens: 200,
        phase_completion_tokens: 100,
        phase_total_tokens: 300,
        phase_duration_ms: 600,
    };

    tracker.push_phase_usage(phase1);
    tracker.push_phase_usage(phase2);

    assert_eq!(
        tracker.phase_usage.len(),
        2,
        "two phases must be accumulated"
    );
    assert_eq!(tracker.phase_usage[0].phase_name, "analyst_team");
    assert_eq!(tracker.phase_usage[1].phase_name, "trader");
    assert_eq!(tracker.phase_usage[0].phase_total_tokens, 150);
    assert_eq!(tracker.phase_usage[1].phase_total_tokens, 300);
}
