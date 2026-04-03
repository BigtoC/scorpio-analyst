use std::sync::Arc;

use graph_flow::{Context, NextAction, Task};
use tempfile::tempdir;

use super::*;
use crate::{
    config::LlmConfig,
    state::{
        AgentTokenUsage, FundamentalData, NewsData, SentimentData, TechnicalData, TradingState,
    },
    workflow::context_bridge::{
        deserialize_state_from_context, serialize_state_to_context, write_prefixed_result,
    },
};

fn sample_state() -> TradingState {
    TradingState::new("AAPL", "2026-03-19")
}

fn sample_llm_config() -> LlmConfig {
    LlmConfig {
        quick_thinking_provider: "openai".to_owned(),
        deep_thinking_provider: "openai".to_owned(),
        quick_thinking_model: "gpt-4o-mini".to_owned(),
        deep_thinking_model: "o3".to_owned(),
        max_debate_rounds: 3,
        max_risk_rounds: 2,
        analyst_timeout_secs: 30,
        retry_max_retries: 3,
        retry_base_delay_ms: 500,
    }
}

async fn seed_state(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("seed state serialization should succeed");
}

async fn context_with_invalid_cached_news() -> Context {
    let ctx = Context::new();
    seed_state(&ctx, &sample_state()).await;
    ctx.set(
        common::KEY_CACHED_NEWS.to_owned(),
        "not valid json".to_owned(),
    )
    .await;
    ctx
}

#[tokio::test]
async fn write_flag_true_readable_back() {
    let ctx = Context::new();
    common::write_flag(&ctx, common::ANALYST_FUNDAMENTAL, true).await;
    let key = format!(
        "{}.{}.{}",
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        common::OK_SUFFIX
    );
    let ok: Option<bool> = ctx.get(&key).await;
    assert_eq!(ok, Some(true));
}

#[tokio::test]
async fn write_err_readable_back() {
    let ctx = Context::new();
    common::write_err(&ctx, common::ANALYST_NEWS, "something went wrong").await;
    let key = format!(
        "{}.{}.{}",
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        common::ERR_SUFFIX
    );
    let msg: Option<String> = ctx.get(&key).await;
    assert_eq!(msg.as_deref(), Some("something went wrong"));
}

#[tokio::test]
async fn analyst_sync_all_succeed_returns_continue() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
            rsi: None,
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store);
    let result = task.run(ctx.clone()).await.expect("task should succeed");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(recovered.fundamental_metrics.is_some());
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());
}

#[tokio::test]
async fn analyst_sync_two_failures_returns_error_instead_of_end() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Two analysts fail.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        false,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        false,
    )
    .await;

    // Remaining analysts succeed.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store);
    let error = task
        .run(ctx)
        .await
        .expect_err("two analyst failures should abort the workflow");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("AnalystSyncTask"));
            assert!(message.contains("2/4 analysts failed"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn analyst_sync_counts_flagged_success_with_unreadable_payload_as_failure() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Fundamental is flagged successful but stores an unreadable payload.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        "analyst.fundamental".to_owned(),
        "not valid json".to_owned(),
    )
    .await;

    // Remaining analysts succeed, so the degraded path should still continue.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
            rsi: None,
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store);
    let result = task.run(ctx.clone()).await.expect("task should succeed");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(
        recovered.fundamental_metrics.is_none(),
        "unreadable payload must not be merged into state"
    );
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());
}

#[tokio::test]
async fn analyst_sync_uses_longest_analyst_latency_for_fan_out_duration() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    for analyst_key in [
        common::ANALYST_FUNDAMENTAL,
        common::ANALYST_SENTIMENT,
        common::ANALYST_NEWS,
        common::ANALYST_TECHNICAL,
    ] {
        ctx.set(
            format!(
                "{}.{}.{}",
                common::ANALYST_PREFIX,
                analyst_key,
                common::OK_SUFFIX
            ),
            true,
        )
        .await;
    }

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
            rsi: None,
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let usages = [
        (
            common::ANALYST_FUNDAMENTAL,
            AgentTokenUsage {
                agent_name: "Fundamental Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                latency_ms: 150,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_SENTIMENT,
            AgentTokenUsage {
                agent_name: "Sentiment Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 11,
                completion_tokens: 6,
                total_tokens: 17,
                latency_ms: 320,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_NEWS,
            AgentTokenUsage {
                agent_name: "News Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
                latency_ms: 240,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_TECHNICAL,
            AgentTokenUsage {
                agent_name: "Technical Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 13,
                completion_tokens: 8,
                total_tokens: 21,
                latency_ms: 90,
                rate_limit_wait_ms: 0,
            },
        ),
    ];

    for (analyst_key, usage) in usages {
        common::write_analyst_usage(&ctx, analyst_key, &usage)
            .await
            .expect("usage write should succeed");
    }

    let task = AnalystSyncTask::new(store);
    task.run(ctx.clone()).await.expect("task should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let phase = recovered
        .token_usage
        .phase_usage
        .iter()
        .find(|phase| phase.phase_name == "Analyst Fan-Out")
        .expect("analyst phase should exist");

    assert_eq!(phase.phase_duration_ms, 320);
}

#[test]
fn task_ids_are_correct() {
    assert_eq!("bearish_researcher", "bearish_researcher");
    assert_eq!("conservative_risk", "conservative_risk");
    assert_eq!("neutral_risk", "neutral_risk");
}

#[tokio::test]
async fn stub_researchers_use_production_role_names() {
    let ctx = Context::new();
    seed_state(&ctx, &sample_state()).await;

    test_helpers::StubBullishResearcherTask
        .run(ctx.clone())
        .await
        .expect("bullish stub should succeed");
    test_helpers::StubBearishResearcherTask
        .run(ctx.clone())
        .await
        .expect("bearish stub should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let roles: Vec<&str> = recovered
        .debate_history
        .iter()
        .map(|message| message.role.as_str())
        .collect();

    assert_eq!(roles, vec!["bullish_researcher", "bearish_researcher"]);
}

#[tokio::test]
async fn stub_risk_moderator_appends_workflow_history_entry() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let mut state = sample_state();
    state
        .risk_discussion_history
        .push(crate::state::DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: "stub: prior risk discussion".to_owned(),
        });
    seed_state(&ctx, &state).await;

    test_helpers::StubRiskModeratorTask {
        snapshot_store: store,
    }
    .run(ctx.clone())
    .await
    .expect("risk moderator stub should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let last_message = recovered
        .risk_discussion_history
        .last()
        .expect("stub should append a moderator history entry");

    assert_eq!(recovered.risk_discussion_history.len(), 2);
    assert_eq!(last_message.role, "risk_moderator");
    assert!(
        !last_message.content.is_empty(),
        "moderator history entry should include synthesis content"
    );
}

#[tokio::test]
async fn sentiment_analyst_invalid_cached_news_fails_closed() {
    let task = SentimentAnalystTask::new(
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::data::FinnhubClient::for_test(),
        sample_llm_config(),
    );
    let ctx = context_with_invalid_cached_news().await;

    let error = task
        .run(ctx)
        .await
        .expect_err("invalid cached news should fail closed");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("SentimentAnalystTask"));
            assert!(message.contains("cached news"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn news_analyst_invalid_cached_news_fails_closed() {
    let task = NewsAnalystTask::new(
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::data::FinnhubClient::for_test(),
        crate::data::FredClient::for_test(),
        sample_llm_config(),
    );
    let ctx = context_with_invalid_cached_news().await;

    let error = task
        .run(ctx)
        .await
        .expect_err("invalid cached news should fail closed");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("NewsAnalystTask"));
            assert!(message.contains("cached news"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}
