use std::sync::Arc;

use graph_flow::{Context, NextAction, Task};
use tempfile::tempdir;

use super::*;
use crate::{
    state::{FundamentalData, NewsData, SentimentData, TechnicalData, TradingState},
    workflow::context_bridge::{
        deserialize_state_from_context, serialize_state_to_context, write_prefixed_result,
    },
};

fn sample_state() -> TradingState {
    TradingState::new("AAPL", "2026-03-19")
}

async fn seed_state(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("seed state serialization should succeed");
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
