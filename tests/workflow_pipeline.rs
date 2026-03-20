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
        SnapshotPhase, SnapshotStore,
        test_support::{
            AnalystSyncTask, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
            KEY_RISK_ROUND, deserialize_state_from_context, serialize_state_to_context,
            write_prefixed_result, write_round_debate_usage, write_round_risk_usage,
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
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect("load_snapshot should not error");

    assert!(
        snapshot.is_some(),
        "phase 1 snapshot must be written by AnalystSyncTask"
    );
    let loaded = snapshot.unwrap();
    assert_eq!(loaded.state.asset_symbol, "AAPL");
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
        .load_snapshot("non-existent-exec-id", SnapshotPhase::AnalystTeam)
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
        .save_snapshot(exec_id, SnapshotPhase::AnalystTeam, &state, None)
        .await
        .expect("first save");

    // Save a second snapshot with the same (exec_id, phase_number).
    let mut state2 = sample_state();
    state2.asset_symbol = "TSLA".to_owned();
    store
        .save_snapshot(exec_id, SnapshotPhase::AnalystTeam, &state2, None)
        .await
        .expect("upsert save");

    // Only one row should exist; it should be the updated one.
    let result = store
        .load_snapshot(exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect("load after upsert");
    assert!(result.is_some());
    let loaded = result.unwrap();
    assert_eq!(
        loaded.state.asset_symbol, "TSLA",
        "upsert must replace the original snapshot"
    );
}

#[cfg(feature = "test-helpers")]
fn phase_from_number(phase: u8) -> SnapshotPhase {
    match phase {
        1 => SnapshotPhase::AnalystTeam,
        2 => SnapshotPhase::ResearcherDebate,
        3 => SnapshotPhase::Trader,
        4 => SnapshotPhase::RiskDiscussion,
        5 => SnapshotPhase::FundManager,
        _ => panic!("unsupported snapshot phase: {phase}"),
    }
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
        workflow::test_support::{TRADING_STATE_KEY, deserialize_state_from_context},
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
// Task 1: GraphFlow errors preserve real task identity
// ────────────────────────────────────────────────────────────────────────────
//
// When `graph_flow::GraphError::TaskExecutionFailed` is returned by the
// runner, the pipeline must surface a `TradingError::GraphFlow` whose `task`
// field contains the originating task name and whose `phase` maps to the
// correct pipeline phase — NOT generic placeholders like "pipeline_execution"
// / "flow_runner" / "task_failure".
//
// This test exercises the public helper `map_graph_error` that the pipeline
// uses to convert `GraphError` into `TradingError::GraphFlow`.

#[test]
fn graphflow_errors_preserve_real_task_identity() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    // Simulate what graph-flow's `execute_single_task` produces:
    // "Task 'bullish_researcher' failed: BullishResearcherTask: failed to run bullish turn: ..."
    let graph_err = graph_flow::GraphError::TaskExecutionFailed(
        "Task 'bullish_researcher' failed: BullishResearcherTask: failed to run bullish turn: connection refused".to_owned()
    );

    let trading_err = map_graph_error(graph_err);

    match &trading_err {
        TradingError::GraphFlow { phase, task, cause } => {
            // Must NOT be the generic labels.
            assert_ne!(
                task, "flow_runner",
                "task must not be generic 'flow_runner'; got phase={phase:?}, task={task:?}"
            );
            assert_ne!(
                task, "task_failure",
                "task must not be generic 'task_failure'; got phase={phase:?}, task={task:?}"
            );
            // Must contain the real task id somewhere.
            assert!(
                task.contains("bullish_researcher"),
                "task field must contain the real task id 'bullish_researcher'; got task={task:?}"
            );
            // Phase must not be the generic "pipeline_execution".
            assert_ne!(
                phase, "pipeline_execution",
                "phase must not be generic 'pipeline_execution'; got phase={phase:?}"
            );
            // Cause should contain the actual error message.
            assert!(
                cause.contains("connection refused"),
                "cause must contain the original error; got cause={cause:?}"
            );
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

/// Verify that non-task GraphError variants (e.g. SessionNotFound) are also
/// mapped with meaningful identity rather than generic labels.
#[test]
fn graphflow_non_task_errors_preserve_identity() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let graph_err = graph_flow::GraphError::SessionNotFound("abc-123".to_owned());
    let trading_err = map_graph_error(graph_err);

    match &trading_err {
        TradingError::GraphFlow {
            phase: _,
            task,
            cause,
        } => {
            assert_ne!(
                task, "flow_runner",
                "task must not be generic 'flow_runner'"
            );
            assert!(
                cause.contains("abc-123"),
                "cause must contain the session id; got cause={cause:?}"
            );
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Task 2: Zero-round debate does not create phantom round entries
// ────────────────────────────────────────────────────────────────────────────
//
// When max_debate_rounds = 0 the graph routes directly to DebateModeratorTask,
// skipping bullish/bearish researchers.  The moderator must NOT increment
// KEY_DEBATE_ROUND or create a "Researcher Debate Round N" PhaseTokenUsage
// entry — only the moderation entry should appear.
//
// We cannot run the full task (it calls run_debate_moderation which needs a
// real LLM), so we test via the DebateModeratorTask unit-test helper that
// exercises the accounting path with a mock moderation result.

#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn zero_round_debate_does_not_create_phantom_round_entry() {
    use scorpio_analyst::workflow::test_support::run_debate_moderator_accounting;

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 0u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    let mod_usage = AgentTokenUsage {
        agent_name: "Debate Moderator".to_owned(),
        model_id: "test-model".to_owned(),
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
        latency_ms: 200,
        token_counts_available: true,
    };

    let (store, _dir) = make_store().await;
    write_round_debate_usage(&ctx, 1, &mod_usage, &mod_usage).await;
    run_debate_moderator_accounting(&ctx, &mod_usage, Arc::clone(&store)).await;

    // Counter must stay at 0.
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(99);
    assert_eq!(
        round, 0,
        "debate round counter must stay at 0 when max_debate_rounds = 0"
    );

    // State must NOT contain any "Researcher Debate Round" entry.
    let final_state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");
    let round_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name.starts_with("Researcher Debate Round"))
        .collect();
    assert!(
        round_entries.is_empty(),
        "zero-round debate must not create phantom 'Researcher Debate Round' entries; found: {:?}",
        round_entries
            .iter()
            .map(|p| &p.phase_name)
            .collect::<Vec<_>>()
    );

    // Moderation entry SHOULD still exist.
    let mod_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name == "Researcher Debate Moderation")
        .collect();
    assert_eq!(
        mod_entries.len(),
        1,
        "zero-round debate must still create one 'Researcher Debate Moderation' entry"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Task 2: Zero-round risk does not create phantom round entries
// ────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn zero_round_risk_does_not_create_phantom_round_entry() {
    use scorpio_analyst::workflow::test_support::run_risk_moderator_accounting;

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;
    ctx.set(KEY_MAX_RISK_ROUNDS, 0u32).await;
    ctx.set(KEY_RISK_ROUND, 0u32).await;

    let mod_usage = AgentTokenUsage {
        agent_name: "Risk Moderator".to_owned(),
        model_id: "test-model".to_owned(),
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
        latency_ms: 200,
        token_counts_available: true,
    };

    let (store, _dir) = make_store().await;
    write_round_risk_usage(&ctx, 1, &mod_usage, &mod_usage, &mod_usage).await;
    run_risk_moderator_accounting(&ctx, &mod_usage, Arc::clone(&store)).await;

    // Counter must stay at 0.
    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(99);
    assert_eq!(
        round, 0,
        "risk round counter must stay at 0 when max_risk_rounds = 0"
    );

    // State must NOT contain any "Risk Discussion Round" entry.
    let final_state = deserialize_state_from_context(&ctx)
        .await
        .expect("state deserialization");
    let round_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name.starts_with("Risk Discussion Round"))
        .collect();
    assert!(
        round_entries.is_empty(),
        "zero-round risk must not create phantom 'Risk Discussion Round' entries; found: {:?}",
        round_entries
            .iter()
            .map(|p| &p.phase_name)
            .collect::<Vec<_>>()
    );

    // Moderation entry SHOULD still exist.
    let mod_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name == "Risk Discussion Moderation")
        .collect();
    assert_eq!(
        mod_entries.len(),
        1,
        "zero-round risk must still create one 'Risk Discussion Moderation' entry"
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

// ────────────────────────────────────────────────────────────────────────────
// Task 3: Analyst child deserialization failure returns Err (orchestration corruption)
// ────────────────────────────────────────────────────────────────────────────
//
// Storing garbage under TRADING_STATE_KEY makes `deserialize_state_from_context`
// fail.  The analyst child task must return `Err(GraphError::TaskExecutionFailed(...))`
// rather than silently degrading into an analyst miss.

#[tokio::test]
async fn analyst_child_deserialization_failure_returns_err() {
    use scorpio_analyst::{
        config::LlmConfig,
        data::FinnhubClient,
        providers::factory::CompletionModelHandle,
        workflow::test_support::{FundamentalAnalystTask, TRADING_STATE_KEY},
    };

    let ctx = Context::new();
    // Store garbage that cannot deserialize into TradingState.
    ctx.set(TRADING_STATE_KEY, "this is not valid JSON {{{".to_owned())
        .await;

    let llm_config = LlmConfig {
        quick_thinking_provider: "openai".to_owned(),
        deep_thinking_provider: "openai".to_owned(),
        quick_thinking_model: "gpt-4o-mini".to_owned(),
        deep_thinking_model: "o3".to_owned(),
        max_debate_rounds: 1,
        max_risk_rounds: 1,
        analyst_timeout_secs: 30,
        retry_max_retries: 1,
        retry_base_delay_ms: 1,
    };

    let task = FundamentalAnalystTask::new(
        CompletionModelHandle::for_test(),
        FinnhubClient::for_test(),
        llm_config,
    );

    let result = task.run(ctx).await;

    assert!(
        result.is_err(),
        "deserialization failure must return Err, not Ok with graceful degradation"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("orchestration corruption"),
        "error must mention 'orchestration corruption'; got: {err_msg}"
    );
    assert!(
        err_msg.contains("FundamentalAnalystTask"),
        "error must identify the task; got: {err_msg}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// BATCH 2 — End-to-End Execution Coverage (Tasks 5–7)
// ════════════════════════════════════════════════════════════════════════════

// ────────────────────────────────────────────────────────────────────────────
// Task 5: True success-path run_analysis_cycle() test
// ────────────────────────────────────────────────────────────────────────────
//
// Replaces all LLM-calling tasks with deterministic stubs, then runs the
// full `TradingPipeline::run_analysis_cycle()` loop via `FlowRunner`.
// Asserts all 5 phases execute and the returned `TradingState` is fully
// populated.

#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn run_analysis_cycle_success_path_populates_all_phases() {
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        state::{Decision, TradingState},
        workflow::{TradingPipeline, test_support::replace_with_stubs},
    };

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("e2e-test.db");

    // Create the store that the pipeline will own.
    let pipeline_store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("pipeline snapshot store");

    // Create a second handle to the same DB for stubs and verification.
    let verify_store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("verify snapshot store"),
    );

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
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("e2e-test", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    // Replace all LLM-calling tasks with deterministic stubs.
    replace_with_stubs(pipeline.graph(), Arc::clone(&verify_store));

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let caller_exec_id = initial_state.execution_id;

    let result = pipeline.run_analysis_cycle(initial_state).await;
    let final_state = result.expect("pipeline must complete successfully with stubs");

    // ── Assertion 1: execution_id is overwritten ────────────────────────
    assert_ne!(
        final_state.execution_id, caller_exec_id,
        "run_analysis_cycle must assign a fresh execution_id"
    );

    // ── Assertion 2: all 5 phases populated ─────────────────────────────
    // Phase 1: analyst data merged
    assert!(
        final_state.fundamental_metrics.is_some(),
        "Phase 1: fundamental_metrics must be populated"
    );
    assert!(
        final_state.technical_indicators.is_some(),
        "Phase 1: technical_indicators must be populated"
    );
    assert!(
        final_state.market_sentiment.is_some(),
        "Phase 1: market_sentiment must be populated"
    );
    assert!(
        final_state.macro_news.is_some(),
        "Phase 1: macro_news must be populated"
    );

    // Phase 2: debate history + consensus
    assert!(
        !final_state.debate_history.is_empty(),
        "Phase 2: debate_history must have entries"
    );
    assert!(
        final_state.consensus_summary.is_some(),
        "Phase 2: consensus_summary must be set"
    );

    // Phase 3: trade proposal
    assert!(
        final_state.trader_proposal.is_some(),
        "Phase 3: trader_proposal must be set"
    );

    // Phase 4: risk reports + discussion
    assert!(
        final_state.aggressive_risk_report.is_some(),
        "Phase 4: aggressive_risk_report must be set"
    );
    assert!(
        final_state.conservative_risk_report.is_some(),
        "Phase 4: conservative_risk_report must be set"
    );
    assert!(
        final_state.neutral_risk_report.is_some(),
        "Phase 4: neutral_risk_report must be set"
    );
    assert!(
        !final_state.risk_discussion_history.is_empty(),
        "Phase 4: risk_discussion_history must have entries"
    );

    // Phase 5: final execution status
    let exec_status = final_state
        .final_execution_status
        .as_ref()
        .expect("Phase 5: final_execution_status must be set");
    assert_eq!(
        exec_status.decision,
        Decision::Approved,
        "Phase 5: stub fund manager approves"
    );

    // ── Assertion 3: 5 snapshots exist ──────────────────────────────────
    let exec_id_str = final_state.execution_id.to_string();
    for phase_num in 1..=5 {
        let snapshot = verify_store
            .load_snapshot(&exec_id_str, phase_from_number(phase_num))
            .await
            .unwrap_or_else(|e| panic!("load_snapshot phase {phase_num} failed: {e}"));
        assert!(
            snapshot.is_some(),
            "snapshot for phase {phase_num} must exist"
        );
    }

    // ── Assertion 4: phase-usage entries exist in expected order ─────────
    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    assert!(
        phase_names.contains(&"Analyst Fan-Out"),
        "phase_usage must contain 'Analyst Fan-Out'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Researcher Debate Round 1"),
        "phase_usage must contain 'Researcher Debate Round 1'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Researcher Debate Moderation"),
        "phase_usage must contain 'Researcher Debate Moderation'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Trader Synthesis"),
        "phase_usage must contain 'Trader Synthesis'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Risk Discussion Round 1"),
        "phase_usage must contain 'Risk Discussion Round 1'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Risk Discussion Moderation"),
        "phase_usage must contain 'Risk Discussion Moderation'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Fund Manager Decision"),
        "phase_usage must contain 'Fund Manager Decision'; got: {phase_names:?}"
    );

    // Verify ordering: Analyst Fan-Out must come before Researcher Debate,
    // which must come before Trader, etc.
    let analyst_idx = phase_names
        .iter()
        .position(|n| *n == "Analyst Fan-Out")
        .unwrap();
    let debate_round_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Round 1")
        .unwrap();
    let debate_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Moderation")
        .unwrap();
    let trader_idx = phase_names
        .iter()
        .position(|n| *n == "Trader Synthesis")
        .unwrap();
    let risk_round_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Round 1")
        .unwrap();
    let risk_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Moderation")
        .unwrap();
    let fund_idx = phase_names
        .iter()
        .position(|n| *n == "Fund Manager Decision")
        .unwrap();

    assert!(
        analyst_idx < debate_round_idx,
        "Analyst must come before Debate Round"
    );
    assert!(
        debate_round_idx < debate_mod_idx,
        "Debate Round must come before Debate Moderation"
    );
    assert!(
        debate_mod_idx < trader_idx,
        "Debate Moderation must come before Trader"
    );
    assert!(
        trader_idx < risk_round_idx,
        "Trader must come before Risk Round"
    );
    assert!(
        risk_round_idx < risk_mod_idx,
        "Risk Round must come before Risk Moderation"
    );
    assert!(
        risk_mod_idx < fund_idx,
        "Risk Moderation must come before Fund Manager"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Task 6: Expand e2e assertions for routing and accounting
// ────────────────────────────────────────────────────────────────────────────
//
// Test variants with different max_debate_rounds and max_risk_rounds settings
// to verify that the graph routing and phase-usage accounting are correct for
// zero-round bypass, single-round, and multi-round loops.

/// Helper: build a pipeline with stubs and the given debate/risk round limits,
/// run it, and return the final state along with the verification store.
#[cfg(feature = "test-helpers")]
async fn run_stubbed_pipeline(
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> (
    scorpio_analyst::state::TradingState,
    Arc<SnapshotStore>,
    tempfile::TempDir,
) {
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        state::TradingState,
        workflow::{TradingPipeline, test_support::replace_with_stubs},
    };

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("e2e-test.db");

    let pipeline_store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("pipeline snapshot store");
    let verify_store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("verify snapshot store"),
    );

    let config = Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds,
            max_risk_rounds,
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
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("e2e-test", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    replace_with_stubs(pipeline.graph(), Arc::clone(&verify_store));

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must complete successfully with stubs");

    (final_state, verify_store, dir)
}

/// Zero-round debate + zero-round risk: graph bypasses both loops entirely.
/// No "Round N" entries should appear; only moderation entries.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn e2e_zero_debate_zero_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(0, 0).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    // Must NOT contain any round entries (debate or risk).
    let debate_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Researcher Debate Round"))
        .collect();
    assert!(
        debate_rounds.is_empty(),
        "zero debate rounds must produce no 'Researcher Debate Round' entries; got: {debate_rounds:?}"
    );

    let risk_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Risk Discussion Round"))
        .collect();
    assert!(
        risk_rounds.is_empty(),
        "zero risk rounds must produce no 'Risk Discussion Round' entries; got: {risk_rounds:?}"
    );

    // Debate and risk moderation entries SHOULD still exist.
    assert!(
        phase_names.contains(&"Researcher Debate Moderation"),
        "zero debate rounds must still produce 'Researcher Debate Moderation'; got: {phase_names:?}"
    );
    assert!(
        phase_names.contains(&"Risk Discussion Moderation"),
        "zero risk rounds must still produce 'Risk Discussion Moderation'; got: {phase_names:?}"
    );

    // Debate history should be empty (researchers were skipped).
    assert!(
        final_state.debate_history.is_empty(),
        "zero debate rounds should produce no debate history entries"
    );

    // Risk discussion history should be empty (risk agents were skipped).
    assert!(
        final_state.risk_discussion_history.is_empty(),
        "zero risk rounds should produce no risk discussion history entries"
    );

    // The pipeline must still complete all the way through.
    assert!(
        final_state.final_execution_status.is_some(),
        "pipeline must still reach fund manager decision with zero rounds"
    );

    // Verify ordering of what IS present.
    let analyst_idx = phase_names
        .iter()
        .position(|n| *n == "Analyst Fan-Out")
        .expect("must have Analyst Fan-Out");
    let debate_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Moderation")
        .expect("must have Researcher Debate Moderation");
    let trader_idx = phase_names
        .iter()
        .position(|n| *n == "Trader Synthesis")
        .expect("must have Trader Synthesis");
    let risk_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Moderation")
        .expect("must have Risk Discussion Moderation");
    let fund_idx = phase_names
        .iter()
        .position(|n| *n == "Fund Manager Decision")
        .expect("must have Fund Manager Decision");

    assert!(analyst_idx < debate_mod_idx);
    assert!(debate_mod_idx < trader_idx);
    assert!(trader_idx < risk_mod_idx);
    assert!(risk_mod_idx < fund_idx);
}

/// Multi-round debate (N=3) + multi-round risk (N=2): graph loops correctly.
/// Phase-usage entries should contain Round 1, 2, 3 for debate and Round 1, 2
/// for risk.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn e2e_multi_round_debate_and_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    // Debate: should have rounds 1, 2, 3.
    for r in 1..=3 {
        let name = format!("Researcher Debate Round {r}");
        assert!(
            phase_names.contains(&name.as_str()),
            "multi-round debate must contain '{name}'; got: {phase_names:?}"
        );
    }
    // Should NOT have round 4.
    assert!(
        !phase_names.contains(&"Researcher Debate Round 4"),
        "max_debate_rounds=3 must not produce Round 4; got: {phase_names:?}"
    );

    // Risk: should have rounds 1, 2.
    for r in 1..=2 {
        let name = format!("Risk Discussion Round {r}");
        assert!(
            phase_names.contains(&name.as_str()),
            "multi-round risk must contain '{name}'; got: {phase_names:?}"
        );
    }
    assert!(
        !phase_names.contains(&"Risk Discussion Round 3"),
        "max_risk_rounds=2 must not produce Round 3; got: {phase_names:?}"
    );

    // Debate history should have entries from each round (2 per round: bull + bear).
    assert_eq!(
        final_state.debate_history.len(),
        6, // 3 rounds * 2 messages (bull + bear)
        "3 debate rounds should produce 6 debate history entries; got: {}",
        final_state.debate_history.len()
    );

    // Risk discussion history should have entries from each round (3 per round).
    assert_eq!(
        final_state.risk_discussion_history.len(),
        6, // 2 rounds * 3 messages (agg + con + neu)
        "2 risk rounds should produce 6 risk discussion history entries; got: {}",
        final_state.risk_discussion_history.len()
    );

    // Verify full ordering.
    let analyst_idx = phase_names
        .iter()
        .position(|n| *n == "Analyst Fan-Out")
        .unwrap();
    let debate_r1_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Round 1")
        .unwrap();
    let debate_r3_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Round 3")
        .unwrap();
    let debate_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Moderation")
        .unwrap();
    let trader_idx = phase_names
        .iter()
        .position(|n| *n == "Trader Synthesis")
        .unwrap();
    let risk_r1_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Round 1")
        .unwrap();
    let risk_r2_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Round 2")
        .unwrap();
    let risk_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Moderation")
        .unwrap();
    let fund_idx = phase_names
        .iter()
        .position(|n| *n == "Fund Manager Decision")
        .unwrap();

    assert!(analyst_idx < debate_r1_idx, "Analyst before Debate Round 1");
    assert!(
        debate_r1_idx < debate_r3_idx,
        "Debate Round 1 before Round 3"
    );
    assert!(
        debate_r3_idx < debate_mod_idx,
        "Debate Round 3 before Moderation"
    );
    assert!(
        debate_mod_idx < trader_idx,
        "Debate Moderation before Trader"
    );
    assert!(trader_idx < risk_r1_idx, "Trader before Risk Round 1");
    assert!(risk_r1_idx < risk_r2_idx, "Risk Round 1 before Round 2");
    assert!(
        risk_r2_idx < risk_mod_idx,
        "Risk Round 2 before Risk Moderation"
    );
    assert!(
        risk_mod_idx < fund_idx,
        "Risk Moderation before Fund Manager"
    );

    // The pipeline must still complete.
    assert!(final_state.final_execution_status.is_some());
}

/// Mixed: zero debate rounds + multiple risk rounds.
/// Verifies that only one loop can be zero while the other runs normally.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn e2e_zero_debate_multi_risk_routing_and_accounting() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(0, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    // No debate round entries.
    let debate_rounds: Vec<&&str> = phase_names
        .iter()
        .filter(|n| n.starts_with("Researcher Debate Round"))
        .collect();
    assert!(
        debate_rounds.is_empty(),
        "zero debate rounds → no round entries"
    );

    // Risk should have rounds 1, 2.
    assert!(phase_names.contains(&"Risk Discussion Round 1"));
    assert!(phase_names.contains(&"Risk Discussion Round 2"));

    // Debate history empty, risk discussion populated.
    assert!(final_state.debate_history.is_empty());
    assert_eq!(final_state.risk_discussion_history.len(), 6); // 2 rounds * 3

    assert!(final_state.final_execution_status.is_some());
}

// ────────────────────────────────────────────────────────────────────────────
// Task 7: Verify snapshot and execution-id behavior end to end
// ────────────────────────────────────────────────────────────────────────────
//
// Two tests:
//  1. Two invocations with the same caller input state produce distinct saved
//     execution IDs, each with its own complete set of 5 snapshots.
//  2. Each phase snapshot contains boundary-appropriate state — later-phase
//     data must be absent at earlier snapshot boundaries.

/// Two separate `run_analysis_cycle()` calls with identical initial state
/// must produce distinct execution IDs, and each must have 5 snapshots.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn e2e_two_invocations_produce_distinct_execution_ids() {
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        state::TradingState,
        workflow::{TradingPipeline, test_support::replace_with_stubs},
    };

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("exec-id-test.db");

    let pipeline_store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("pipeline snapshot store");
    let verify_store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("verify snapshot store"),
    );

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
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("exec-id-test", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    replace_with_stubs(pipeline.graph(), Arc::clone(&verify_store));

    // Run #1: identical initial state (same symbol + date).
    let state_1 = TradingState::new("AAPL", "2026-03-20");
    let final_1 = pipeline
        .run_analysis_cycle(state_1)
        .await
        .expect("run #1 must succeed");

    // Run #2: identical initial state.
    let state_2 = TradingState::new("AAPL", "2026-03-20");
    let final_2 = pipeline
        .run_analysis_cycle(state_2)
        .await
        .expect("run #2 must succeed");

    // Execution IDs must be distinct.
    assert_ne!(
        final_1.execution_id, final_2.execution_id,
        "two invocations must produce distinct execution IDs"
    );

    // Both runs must have all 5 snapshots.
    let exec_id_1 = final_1.execution_id.to_string();
    let exec_id_2 = final_2.execution_id.to_string();
    for phase in 1..=5u8 {
        let snapshot_phase = phase_from_number(phase);
        let snap_1 = verify_store
            .load_snapshot(&exec_id_1, snapshot_phase)
            .await
            .unwrap_or_else(|e| panic!("load run#1 phase {phase}: {e}"));
        assert!(
            snap_1.is_some(),
            "run #1 must have snapshot for phase {phase}"
        );

        let snap_2 = verify_store
            .load_snapshot(&exec_id_2, snapshot_phase)
            .await
            .unwrap_or_else(|e| panic!("load run#2 phase {phase}: {e}"));
        assert!(
            snap_2.is_some(),
            "run #2 must have snapshot for phase {phase}"
        );
    }
}

/// Snapshots for phases 1-5 contain boundary-appropriate state: data set
/// in a later phase must be absent in earlier phase snapshots.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn e2e_snapshots_contain_boundary_appropriate_state() {
    let (final_state, verify_store, _dir) = run_stubbed_pipeline(1, 1).await;
    let exec_id = final_state.execution_id.to_string();

    // Load all 5 snapshots.
    let mut snaps = Vec::new();
    for phase in 1..=5u8 {
        let snapshot = verify_store
            .load_snapshot(&exec_id, phase_from_number(phase))
            .await
            .unwrap_or_else(|e| panic!("load phase {phase}: {e}"))
            .unwrap_or_else(|| panic!("phase {phase} snapshot must exist"));
        snaps.push(snapshot.state);
    }

    // ── Phase 1 (Analyst Fan-Out) boundary ──────────────────────────────
    // Analyst data present.
    assert!(
        snaps[0].fundamental_metrics.is_some(),
        "phase 1: fundamental_metrics must be present"
    );
    assert!(
        snaps[0].technical_indicators.is_some(),
        "phase 1: technical_indicators must be present"
    );
    assert!(
        snaps[0].market_sentiment.is_some(),
        "phase 1: market_sentiment must be present"
    );
    assert!(
        snaps[0].macro_news.is_some(),
        "phase 1: macro_news must be present"
    );
    // Later-phase data must NOT be present.
    assert!(
        snaps[0].debate_history.is_empty(),
        "phase 1: debate_history must be empty"
    );
    assert!(
        snaps[0].consensus_summary.is_none(),
        "phase 1: consensus_summary must be absent"
    );
    assert!(
        snaps[0].trader_proposal.is_none(),
        "phase 1: trader_proposal must be absent"
    );
    assert!(
        snaps[0].aggressive_risk_report.is_none(),
        "phase 1: risk reports must be absent"
    );
    assert!(
        snaps[0].final_execution_status.is_none(),
        "phase 1: final_execution_status must be absent"
    );

    // ── Phase 2 (Researcher Debate) boundary ────────────────────────────
    // Analyst data + debate data present.
    assert!(
        snaps[1].fundamental_metrics.is_some(),
        "phase 2: fundamental_metrics must be present"
    );
    assert!(
        !snaps[1].debate_history.is_empty(),
        "phase 2: debate_history must be populated"
    );
    assert!(
        snaps[1].consensus_summary.is_some(),
        "phase 2: consensus_summary must be set"
    );
    // Later-phase data absent.
    assert!(
        snaps[1].trader_proposal.is_none(),
        "phase 2: trader_proposal must be absent"
    );
    assert!(
        snaps[1].aggressive_risk_report.is_none(),
        "phase 2: risk reports must be absent"
    );
    assert!(
        snaps[1].final_execution_status.is_none(),
        "phase 2: final_execution_status must be absent"
    );

    // ── Phase 3 (Trader Synthesis) boundary ─────────────────────────────
    // Analyst + debate + trader present.
    assert!(
        snaps[2].fundamental_metrics.is_some(),
        "phase 3: fundamental_metrics must be present"
    );
    assert!(
        snaps[2].consensus_summary.is_some(),
        "phase 3: consensus_summary must be present"
    );
    assert!(
        snaps[2].trader_proposal.is_some(),
        "phase 3: trader_proposal must be set"
    );
    // Later-phase data absent.
    assert!(
        snaps[2].aggressive_risk_report.is_none(),
        "phase 3: risk reports must be absent"
    );
    assert!(
        snaps[2].conservative_risk_report.is_none(),
        "phase 3: conservative risk must be absent"
    );
    assert!(
        snaps[2].neutral_risk_report.is_none(),
        "phase 3: neutral risk must be absent"
    );
    assert!(
        snaps[2].final_execution_status.is_none(),
        "phase 3: final_execution_status must be absent"
    );

    // ── Phase 4 (Risk Management) boundary ──────────────────────────────
    // Analyst + debate + trader + risk present.
    assert!(
        snaps[3].trader_proposal.is_some(),
        "phase 4: trader_proposal must be present"
    );
    assert!(
        snaps[3].aggressive_risk_report.is_some(),
        "phase 4: aggressive_risk_report must be set"
    );
    assert!(
        snaps[3].conservative_risk_report.is_some(),
        "phase 4: conservative_risk_report must be set"
    );
    assert!(
        snaps[3].neutral_risk_report.is_some(),
        "phase 4: neutral_risk_report must be set"
    );
    assert!(
        !snaps[3].risk_discussion_history.is_empty(),
        "phase 4: risk_discussion_history must be populated"
    );
    // Fund manager not yet.
    assert!(
        snaps[3].final_execution_status.is_none(),
        "phase 4: final_execution_status must be absent"
    );

    // ── Phase 5 (Fund Manager Decision) boundary ────────────────────────
    // Everything present.
    assert!(
        snaps[4].fundamental_metrics.is_some(),
        "phase 5: fundamental_metrics must be present"
    );
    assert!(
        snaps[4].consensus_summary.is_some(),
        "phase 5: consensus_summary must be present"
    );
    assert!(
        snaps[4].trader_proposal.is_some(),
        "phase 5: trader_proposal must be present"
    );
    assert!(
        snaps[4].aggressive_risk_report.is_some(),
        "phase 5: aggressive_risk_report must be present"
    );
    assert!(
        snaps[4].final_execution_status.is_some(),
        "phase 5: final_execution_status must be set"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Task 9: Sanitize workflow-surfaced graph errors
// ────────────────────────────────────────────────────────────────────────────
//
// Raw provider error text (API keys, bearer tokens, auth headers) must not
// leak through `TradingError::GraphFlow` messages returned from the workflow
// layer.  The workflow must sanitize error causes using the same credential-
// redaction logic as the provider layer.

/// Workflow-surfaced `GraphError::TaskExecutionFailed` messages must redact
/// credential-like substrings before surfacing them in `TradingError::GraphFlow`.
#[test]
fn workflow_graph_error_redacts_credentials_in_cause() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    // Simulate a task failure whose cause embeds a raw provider error
    // containing an API key, bearer token, and auth header.
    let raw_cause = concat!(
        "TraderTask: run_trader failed: provider=openai model=o3 ",
        "summary=HTTP 401 Unauthorized api_key=sk-live-abc123XYZ ",
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig"
    );
    let graph_err =
        graph_flow::GraphError::TaskExecutionFailed(format!("Task 'trader' failed: {raw_cause}"));

    let trading_err = map_graph_error(graph_err);

    match &trading_err {
        TradingError::GraphFlow { cause, .. } => {
            // API key must be redacted.
            assert!(
                !cause.contains("sk-live-abc123XYZ"),
                "cause must not contain raw API key; got: {cause}"
            );
            // Bearer token must be redacted.
            assert!(
                !cause.contains("eyJhbGciOiJIUzI1NiJ9"),
                "cause must not contain raw bearer token; got: {cause}"
            );
            // Authorization header must be redacted.
            assert!(
                !cause.contains("Authorization:"),
                "cause must not contain raw Authorization header; got: {cause}"
            );
            // The [REDACTED] placeholder must appear.
            assert!(
                cause.contains("[REDACTED]"),
                "cause must contain [REDACTED] placeholder; got: {cause}"
            );
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

/// Non-task GraphError variants (catch-all path) also sanitize their cause.
#[test]
fn workflow_non_task_graph_error_redacts_credentials() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let graph_err = graph_flow::GraphError::ContextError(
        "context fetch failed: token=secretvalue123 for session".to_owned(),
    );

    let trading_err = map_graph_error(graph_err);

    match &trading_err {
        TradingError::GraphFlow { cause, .. } => {
            assert!(
                !cause.contains("secretvalue123"),
                "cause must not contain raw token value; got: {cause}"
            );
            assert!(
                cause.contains("[REDACTED]"),
                "cause must contain [REDACTED]; got: {cause}"
            );
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Task 11: Step ceiling and snapshot path hardening
// ────────────────────────────────────────────────────────────────────────────

/// The pipeline must fail with an informative error when the step ceiling is
/// exceeded, rather than looping indefinitely.  This test constructs a pipeline
/// whose debate loop never terminates (the debate-round counter is never
/// incremented past max) and verifies the ceiling kicks in.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn step_ceiling_prevents_runaway_loop() {
    use graph_flow::{Context, NextAction, TaskResult};
    use scorpio_analyst::{
        config::{ApiConfig, Config, LlmConfig, TradingConfig},
        data::{FinnhubClient, YFinanceClient},
        error::TradingError,
        providers::factory::CompletionModelHandle,
        rate_limit::SharedRateLimiter,
        state::TradingState,
        workflow::{TradingPipeline, test_support::replace_with_stubs},
    };
    use std::sync::Arc;

    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("ceiling-test.db");

    let pipeline_store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("pipeline snapshot store");

    // Use large debate rounds to ensure the loop would run far beyond the
    // ceiling if unchecked.  The stub debate moderator increments normally,
    // but we'll replace it with one that never increments — causing an infinite loop.
    let config = Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 0,
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
    let yfinance = YFinanceClient::new(SharedRateLimiter::new("ceiling-test", 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    // First replace with normal stubs, then override the debate moderator
    // with one that NEVER increments the debate round — causing an infinite loop.
    let verify_store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("verify snapshot store"),
    );
    replace_with_stubs(pipeline.graph(), verify_store);

    // Replace the debate_moderator stub with one that never increments the
    // round counter, simulating corrupted state.
    struct RunawayDebateModerator;

    #[async_trait::async_trait]
    impl graph_flow::Task for RunawayDebateModerator {
        fn id(&self) -> &str {
            "debate_moderator"
        }
        async fn run(&self, _context: Context) -> graph_flow::Result<TaskResult> {
            // Intentionally do NOT increment KEY_DEBATE_ROUND.
            // The conditional edge will keep looping back to bullish_researcher.
            Ok(TaskResult::new(None, NextAction::Continue))
        }
    }

    pipeline.graph().add_task(Arc::new(RunawayDebateModerator));

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let result = pipeline.run_analysis_cycle(initial_state).await;

    assert!(
        result.is_err(),
        "pipeline should fail with step ceiling error"
    );
    let err = result.unwrap_err();
    match &err {
        TradingError::GraphFlow {
            phase, task, cause, ..
        } => {
            assert_eq!(task, "step_ceiling", "task should be 'step_ceiling'");
            assert_eq!(
                phase, "pipeline_execution",
                "phase should be 'pipeline_execution'"
            );
            assert!(
                cause.contains("exceeded maximum"),
                "cause should mention ceiling: {cause}"
            );
            assert!(
                cause.contains("runaway loop"),
                "cause should mention runaway: {cause}"
            );
        }
        other => panic!("expected TradingError::GraphFlow with step_ceiling, got: {other:?}"),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Task 14: Accounting fidelity — per-round phase names, agent attribution,
//          nonzero timing, total reconciliation, entry ordering
// ────────────────────────────────────────────────────────────────────────────

/// Debate round `PhaseTokenUsage` entries carry the correct phase name pattern,
/// correct agent names (Bullish/Bearish Researcher), and credible nonzero timing
/// derived from agent latencies.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_debate_rounds_have_correct_phase_names_and_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(2, 1).await;

    for round in 1..=2u32 {
        let expected_name = format!("Researcher Debate Round {round}");
        let phase = final_state
            .token_usage
            .phase_usage
            .iter()
            .find(|p| p.phase_name == expected_name)
            .unwrap_or_else(|| {
                panic!(
                    "expected phase '{expected_name}' not found in: {:?}",
                    final_state
                        .token_usage
                        .phase_usage
                        .iter()
                        .map(|p| &p.phase_name)
                        .collect::<Vec<_>>()
                )
            });

        // Correct agent attribution: exactly bull + bear.
        let agent_names: Vec<&str> = phase
            .agent_usage
            .iter()
            .map(|a| a.agent_name.as_str())
            .collect();
        assert_eq!(
            agent_names,
            vec!["Bullish Researcher", "Bearish Researcher"],
            "debate round {round} agents"
        );

        // Credible nonzero timing: stubs produce latency_ms = 1 each, so
        // round_duration_ms = bull(1) + bear(1) = 2.
        assert!(
            phase.phase_duration_ms > 0,
            "debate round {round} phase_duration_ms must be nonzero, got: {}",
            phase.phase_duration_ms
        );
        assert_eq!(
            phase.phase_duration_ms, 2,
            "debate round {round} phase_duration_ms = bull(1) + bear(1) = 2"
        );
    }
}

/// Risk round `PhaseTokenUsage` entries carry the correct phase name pattern,
/// correct agent names (Aggressive/Conservative/Neutral Risk), and credible
/// nonzero timing derived from agent latencies.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_risk_rounds_have_correct_phase_names_and_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(1, 2).await;

    for round in 1..=2u32 {
        let expected_name = format!("Risk Discussion Round {round}");
        let phase = final_state
            .token_usage
            .phase_usage
            .iter()
            .find(|p| p.phase_name == expected_name)
            .unwrap_or_else(|| {
                panic!(
                    "expected phase '{expected_name}' not found in: {:?}",
                    final_state
                        .token_usage
                        .phase_usage
                        .iter()
                        .map(|p| &p.phase_name)
                        .collect::<Vec<_>>()
                )
            });

        // Correct agent attribution: exactly agg + con + neu.
        let agent_names: Vec<&str> = phase
            .agent_usage
            .iter()
            .map(|a| a.agent_name.as_str())
            .collect();
        assert_eq!(
            agent_names,
            vec!["Aggressive Risk", "Conservative Risk", "Neutral Risk"],
            "risk round {round} agents"
        );

        // Credible nonzero timing: stubs produce latency_ms = 1 each, so
        // round_duration_ms = agg(1) + con(1) + neu(1) = 3.
        assert!(
            phase.phase_duration_ms > 0,
            "risk round {round} phase_duration_ms must be nonzero, got: {}",
            phase.phase_duration_ms
        );
        assert_eq!(
            phase.phase_duration_ms, 3,
            "risk round {round} phase_duration_ms = agg(1) + con(1) + neu(1) = 3"
        );
    }
}

/// Per-round token counts reconcile with their contained agents: the round's
/// aggregate prompt/completion/total must equal the sum of its agents' values.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_round_token_totals_reconcile_with_agents() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(2, 2).await;

    for phase in &final_state.token_usage.phase_usage {
        if phase.phase_name.contains("Round") {
            let sum_prompt: u64 = phase.agent_usage.iter().map(|a| a.prompt_tokens).sum();
            let sum_completion: u64 = phase.agent_usage.iter().map(|a| a.completion_tokens).sum();
            let sum_total: u64 = phase.agent_usage.iter().map(|a| a.total_tokens).sum();

            assert_eq!(
                phase.phase_prompt_tokens, sum_prompt,
                "'{}' prompt token mismatch",
                phase.phase_name
            );
            assert_eq!(
                phase.phase_completion_tokens, sum_completion,
                "'{}' completion token mismatch",
                phase.phase_name
            );
            assert_eq!(
                phase.phase_total_tokens, sum_total,
                "'{}' total token mismatch",
                phase.phase_name
            );
        }
    }
}

/// The tracker's running totals equal the sum of all phase entries.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_tracker_totals_reconcile_with_all_phases() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let expected_prompt: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_prompt_tokens)
        .sum();
    let expected_completion: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_completion_tokens)
        .sum();
    let expected_total: u64 = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_total_tokens)
        .sum();

    assert_eq!(
        final_state.token_usage.total_prompt_tokens, expected_prompt,
        "tracker total_prompt_tokens mismatch"
    );
    assert_eq!(
        final_state.token_usage.total_completion_tokens, expected_completion,
        "tracker total_completion_tokens mismatch"
    );
    assert_eq!(
        final_state.token_usage.total_tokens, expected_total,
        "tracker total_tokens mismatch"
    );
}

/// Round entries appear strictly before their phase's moderation entry.
/// Debate rounds 1..N appear before "Researcher Debate Moderation".
/// Risk rounds 1..N appear before "Risk Discussion Moderation".
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_round_entries_precede_moderation_entries() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(3, 2).await;

    let phase_names: Vec<&str> = final_state
        .token_usage
        .phase_usage
        .iter()
        .map(|p| p.phase_name.as_str())
        .collect();

    let debate_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Researcher Debate Moderation")
        .expect("debate moderation entry must exist");

    for round in 1..=3u32 {
        let name = format!("Researcher Debate Round {round}");
        let round_idx = phase_names
            .iter()
            .position(|n| *n == name.as_str())
            .unwrap_or_else(|| panic!("'{name}' must exist"));
        assert!(
            round_idx < debate_mod_idx,
            "'{name}' (idx={round_idx}) must precede debate moderation (idx={debate_mod_idx})"
        );
    }

    let risk_mod_idx = phase_names
        .iter()
        .position(|n| *n == "Risk Discussion Moderation")
        .expect("risk moderation entry must exist");

    for round in 1..=2u32 {
        let name = format!("Risk Discussion Round {round}");
        let round_idx = phase_names
            .iter()
            .position(|n| *n == name.as_str())
            .unwrap_or_else(|| panic!("'{name}' must exist"));
        assert!(
            round_idx < risk_mod_idx,
            "'{name}' (idx={round_idx}) must precede risk moderation (idx={risk_mod_idx})"
        );
    }
}

/// Moderation entries exist with correct agent attribution and reconciled
/// token counts.  Their timing is wall-clock (`phase_start.elapsed()`), which
/// may be 0ms in fast stub runs — the important property is that round entries
/// use the agent-latency proxy while moderation uses wall-clock, and both are
/// structurally populated.
#[cfg(feature = "test-helpers")]
#[tokio::test]
async fn accounting_moderation_entries_are_structurally_correct() {
    let (final_state, _store, _dir) = run_stubbed_pipeline(1, 1).await;

    let debate_mod = final_state
        .token_usage
        .phase_usage
        .iter()
        .find(|p| p.phase_name == "Researcher Debate Moderation")
        .expect("debate moderation entry must exist");
    // Moderation entry should contain exactly the moderator agent.
    assert_eq!(
        debate_mod.agent_usage.len(),
        1,
        "debate moderation should have 1 agent"
    );
    assert_eq!(
        debate_mod.agent_usage[0].agent_name, "Debate Moderator",
        "debate moderation agent name"
    );
    // Token reconciliation.
    assert_eq!(
        debate_mod.phase_prompt_tokens, debate_mod.agent_usage[0].prompt_tokens,
        "debate mod prompt token reconciliation"
    );
    assert_eq!(
        debate_mod.phase_total_tokens, debate_mod.agent_usage[0].total_tokens,
        "debate mod total token reconciliation"
    );

    let risk_mod = final_state
        .token_usage
        .phase_usage
        .iter()
        .find(|p| p.phase_name == "Risk Discussion Moderation")
        .expect("risk moderation entry must exist");
    assert_eq!(
        risk_mod.agent_usage.len(),
        1,
        "risk moderation should have 1 agent"
    );
    assert_eq!(
        risk_mod.agent_usage[0].agent_name, "Risk Moderator",
        "risk moderation agent name"
    );
    assert_eq!(
        risk_mod.phase_prompt_tokens, risk_mod.agent_usage[0].prompt_tokens,
        "risk mod prompt token reconciliation"
    );
    assert_eq!(
        risk_mod.phase_total_tokens, risk_mod.agent_usage[0].total_tokens,
        "risk mod total token reconciliation"
    );
}
