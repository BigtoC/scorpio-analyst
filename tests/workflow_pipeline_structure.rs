#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use std::path::Path;
use std::sync::Arc;

use graph_flow::{Context, NextAction, Task};
use scorpio_analyst::{
    data::FredClient,
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
use workflow_pipeline_make_pipeline::make_pipeline;

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

#[test]
fn pipeline_build_graph_produces_graph_without_panic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pipeline, _store, _dir) = rt.block_on(make_pipeline("test.db", "test-yfinance", 1, 1));
    let _graph = pipeline.build_graph();
}

#[test]
fn pipeline_build_graph_produces_graph_without_fred_env_key() {
    let _ = FredClient::for_test();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pipeline, _store, _dir) =
        rt.block_on(make_pipeline("test-no-fred.db", "test-yfinance", 1, 1));
    let _graph = pipeline.build_graph();
}

#[test]
fn workflow_support_modules_live_under_support_subdir() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");

    for helper in [
        "workflow_pipeline_make_pipeline.rs",
        "workflow_pipeline_e2e_support.rs",
        "workflow_pipeline_stubbed_support.rs",
        "workflow_observability_collectors.rs",
        "workflow_observability_task_support.rs",
        "workflow_observability_pipeline_support.rs",
    ] {
        assert!(
            tests_dir.join("support").join(helper).exists(),
            "expected `tests/support/{helper}` to exist"
        );
        assert!(
            !tests_dir.join(helper).exists(),
            "did not expect `tests/{helper}` to remain at the tests root"
        );
    }
}

#[tokio::test]
async fn integration_one_analyst_failure_pipeline_continues() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

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

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(recovered.fundamental_metrics.is_none());
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());
}

#[tokio::test]
async fn integration_two_analyst_failures_abort_pipeline() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    set_analyst_ok(&ctx, "fundamental", false).await;
    set_analyst_ok(&ctx, "sentiment", false).await;
    set_analyst_err(&ctx, "fundamental", "error 1").await;
    set_analyst_err(&ctx, "sentiment", "error 2").await;
    set_analyst_ok(&ctx, "news", true).await;
    set_analyst_ok(&ctx, "technical", true).await;
    write_analyst_data(&ctx, "news", news_data()).await;
    write_analyst_data(&ctx, "technical", technical_data()).await;

    let task = AnalystSyncTask::new(store);
    let error = task
        .run(ctx)
        .await
        .expect_err("task should fail when two analysts fail");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("AnalystSyncTask"));
            assert!(message.contains("2/4 analysts failed"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn integration_debate_round_counter_drives_cycling() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 2u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);
    assert!(round < max);

    ctx.set(KEY_DEBATE_ROUND, 1u32).await;
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    assert!(round < max);

    ctx.set(KEY_DEBATE_ROUND, 2u32).await;
    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    assert!(round >= max);
}

#[tokio::test]
async fn integration_risk_round_counter_drives_cycling() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_RISK_ROUNDS, 2u32).await;
    ctx.set(KEY_RISK_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);
    assert!(round < max);

    ctx.set(KEY_RISK_ROUND, 2u32).await;
    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    assert!(round >= max);
}

#[tokio::test]
async fn integration_zero_risk_rounds_bypasses_loop() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_RISK_ROUNDS, 0u32).await;
    ctx.set(KEY_RISK_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);
    assert!(round >= max);
}

#[tokio::test]
async fn integration_phase_snapshot_written_and_readable() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;
    seed_all_analysts_ok(&ctx).await;

    let task = AnalystSyncTask::new(Arc::clone(&store));
    let result = task.run(ctx.clone()).await.expect("task must succeed");
    assert_eq!(result.next_action, NextAction::Continue);

    let final_state = deserialize_state_from_context(&ctx).await.unwrap();
    let exec_id = final_state.execution_id.to_string();

    let snapshot = store
        .load_snapshot(&exec_id, SnapshotPhase::AnalystTeam)
        .await
        .expect("load_snapshot should not error");

    assert!(snapshot.is_some());
    assert_eq!(snapshot.unwrap().state.asset_symbol, "AAPL");
}

#[tokio::test]
async fn integration_token_usage_accumulated_after_analyst_sync() {
    let (store, _dir) = make_store().await;
    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;
    seed_all_analysts_ok(&ctx).await;

    let task = AnalystSyncTask::new(store);
    let result = task.run(ctx.clone()).await.expect("task must succeed");
    assert_eq!(result.next_action, NextAction::Continue);

    let final_state = deserialize_state_from_context(&ctx).await.unwrap();
    let phase_usages = &final_state.token_usage.phase_usage;
    assert!(!phase_usages.is_empty());

    let analyst_phase = phase_usages
        .iter()
        .find(|p| p.phase_name == "Analyst Fan-Out")
        .expect("analyst phase should exist");
    assert_eq!(analyst_phase.agent_usage.len(), 4);
}

#[tokio::test]
async fn snapshot_store_returns_none_for_unknown_execution_id() {
    let (store, _dir) = make_store().await;
    let result = store
        .load_snapshot("non-existent-exec-id", SnapshotPhase::AnalystTeam)
        .await
        .expect("load_snapshot must not error for missing row");
    assert!(result.is_none());
}

#[tokio::test]
async fn snapshot_store_upsert_replaces_existing_snapshot() {
    let (store, _dir) = make_store().await;
    let state = sample_state();

    store
        .save_snapshot(
            "upsert-test-exec-id",
            SnapshotPhase::AnalystTeam,
            &state,
            None,
        )
        .await
        .expect("first save");

    let mut state2 = sample_state();
    state2.asset_symbol = "TSLA".to_owned();
    store
        .save_snapshot(
            "upsert-test-exec-id",
            SnapshotPhase::AnalystTeam,
            &state2,
            None,
        )
        .await
        .expect("upsert save");

    let result = store
        .load_snapshot("upsert-test-exec-id", SnapshotPhase::AnalystTeam)
        .await
        .expect("load after upsert");
    assert_eq!(result.unwrap().state.asset_symbol, "TSLA");
}

#[test]
fn pipeline_graph_topology_has_correct_start_and_all_nodes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pipeline, _store, _dir) = rt.block_on(make_pipeline("topology.db", "test-topology", 1, 1));
    let graph = pipeline.build_graph();

    assert_eq!(graph.start_task_id(), Some("analyst_fanout".to_owned()));

    for id in [
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
    ] {
        assert!(
            graph.get_task(id).is_some(),
            "graph must contain node '{id}'"
        );
    }
}

#[test]
fn replace_task_for_test_rejects_unknown_task_id_with_typed_error() {
    struct UnknownTask;

    #[async_trait::async_trait]
    impl graph_flow::Task for UnknownTask {
        fn id(&self) -> &str {
            "not_a_pipeline_task"
        }

        async fn run(&self, _context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
            Ok(graph_flow::TaskResult::new(
                None,
                graph_flow::NextAction::Continue,
            ))
        }
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (pipeline, _store, _dir) = rt.block_on(make_pipeline(
        "invalid-replace.db",
        "test-invalid-replace",
        1,
        1,
    ));

    let err = pipeline
        .replace_task_for_test(Arc::new(UnknownTask))
        .expect_err("unknown ids must be rejected via Result");
    assert!(err.to_string().contains("not_a_pipeline_task"));
}

#[tokio::test]
async fn build_graph_returns_detached_graph_not_live_pipeline_graph() {
    struct FailingDebateModerator;

    #[async_trait::async_trait]
    impl graph_flow::Task for FailingDebateModerator {
        fn id(&self) -> &str {
            "debate_moderator"
        }

        async fn run(&self, _context: Context) -> graph_flow::Result<graph_flow::TaskResult> {
            Err(graph_flow::GraphError::TaskExecutionFailed(
                "detached test graph should not affect pipeline".to_owned(),
            ))
        }
    }

    let (pipeline, _store, _dir) =
        make_pipeline("detached-graph.db", "test-detached-graph", 1, 1).await;
    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let graph = pipeline.build_graph();
    graph.add_task(Arc::new(FailingDebateModerator));

    let result = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn integration_zero_debate_rounds_bypasses_loop() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 0u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
    let max: u32 = ctx.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);
    assert!(round >= max);
}

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

    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[test]
fn execution_ids_are_distinct_across_cycles() {
    let state1 = sample_state();
    let state2 = sample_state();
    assert_ne!(state1.execution_id, state2.execution_id);
}

#[test]
fn graphflow_errors_preserve_real_task_identity() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let graph_err = graph_flow::GraphError::TaskExecutionFailed(
        "Task 'bullish_researcher' failed: BullishResearcherTask: failed to run bullish turn: connection refused".to_owned(),
    );

    let trading_err = map_graph_error(graph_err);
    match &trading_err {
        TradingError::GraphFlow { phase, task, cause } => {
            assert_ne!(task, "flow_runner");
            assert_ne!(task, "task_failure");
            assert!(task.contains("bullish_researcher"));
            assert_ne!(phase, "pipeline_execution");
            assert!(cause.contains("connection refused"));
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

#[test]
fn graphflow_non_task_errors_preserve_identity() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let graph_err = graph_flow::GraphError::SessionNotFound("abc-123".to_owned());
    let trading_err = map_graph_error(graph_err);

    match &trading_err {
        TradingError::GraphFlow { task, cause, .. } => {
            assert_ne!(task, "flow_runner");
            assert!(cause.contains("abc-123"));
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

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
        rate_limit_wait_ms: 0,
    };

    let (store, _dir) = make_store().await;
    write_round_debate_usage(&ctx, 1, &mod_usage, &mod_usage).await;
    run_debate_moderator_accounting(&ctx, &mod_usage, Arc::clone(&store)).await;

    let round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(99);
    assert_eq!(round, 0);

    let final_state = deserialize_state_from_context(&ctx).await.unwrap();
    let round_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name.starts_with("Researcher Debate Round"))
        .collect();
    assert!(round_entries.is_empty());

    let mod_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name == "Researcher Debate Moderation")
        .collect();
    assert_eq!(mod_entries.len(), 1);
}

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
        rate_limit_wait_ms: 0,
    };

    let (store, _dir) = make_store().await;
    write_round_risk_usage(&ctx, 1, &mod_usage, &mod_usage, &mod_usage).await;
    run_risk_moderator_accounting(&ctx, &mod_usage, Arc::clone(&store)).await;

    let round: u32 = ctx.get(KEY_RISK_ROUND).await.unwrap_or(99);
    assert_eq!(round, 0);

    let final_state = deserialize_state_from_context(&ctx).await.unwrap();
    let round_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name.starts_with("Risk Discussion Round"))
        .collect();
    assert!(round_entries.is_empty());

    let mod_entries: Vec<_> = final_state
        .token_usage
        .phase_usage
        .iter()
        .filter(|p| p.phase_name == "Risk Discussion Moderation")
        .collect();
    assert_eq!(mod_entries.len(), 1);
}

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
            rate_limit_wait_ms: 0,
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
            rate_limit_wait_ms: 0,
        }],
        phase_prompt_tokens: 200,
        phase_completion_tokens: 100,
        phase_total_tokens: 300,
        phase_duration_ms: 600,
    };

    tracker.push_phase_usage(phase1);
    tracker.push_phase_usage(phase2);

    assert_eq!(tracker.phase_usage.len(), 2);
    assert_eq!(tracker.phase_usage[0].phase_name, "analyst_team");
    assert_eq!(tracker.phase_usage[1].phase_name, "trader");
    assert_eq!(tracker.phase_usage[0].phase_total_tokens, 150);
    assert_eq!(tracker.phase_usage[1].phase_total_tokens, 300);
}

#[tokio::test]
async fn analyst_child_deserialization_failure_returns_err() {
    use scorpio_analyst::{
        config::LlmConfig,
        data::FinnhubClient,
        providers::factory::CompletionModelHandle,
        workflow::test_support::{FundamentalAnalystTask, TRADING_STATE_KEY},
    };

    let ctx = Context::new();
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
    assert!(result.is_err());

    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("orchestration corruption"));
    assert!(err_msg.contains("FundamentalAnalystTask"));
}

#[test]
fn workflow_graph_error_redacts_credentials_in_cause() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let raw_cause = concat!(
        "TraderTask: run_trader failed: provider=openai model=o3 ",
        "summary=HTTP 401 Unauthorized api_key=sk-live-abc123XYZ ",
        "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
    );
    let graph_err =
        graph_flow::GraphError::TaskExecutionFailed(format!("Task 'trader' failed: {raw_cause}"));

    let trading_err = map_graph_error(graph_err);
    match &trading_err {
        TradingError::GraphFlow { cause, .. } => {
            assert!(!cause.contains("sk-live-abc123XYZ"));
            assert!(!cause.contains("eyJhbGciOiJIUzI1NiJ9"));
            assert!(!cause.contains("Authorization:"));
            assert!(cause.contains("[REDACTED]"));
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}

#[test]
fn workflow_non_task_graph_error_redacts_credentials() {
    use scorpio_analyst::{error::TradingError, workflow::test_support::map_graph_error};

    let graph_err = graph_flow::GraphError::ContextError(
        "context fetch failed: token=secretvalue123 for session".to_owned(),
    );

    let trading_err = map_graph_error(graph_err);
    match &trading_err {
        TradingError::GraphFlow { cause, .. } => {
            assert!(!cause.contains("secretvalue123"));
            assert!(cause.contains("[REDACTED]"));
        }
        other => panic!("expected TradingError::GraphFlow, got: {other:?}"),
    }
}
