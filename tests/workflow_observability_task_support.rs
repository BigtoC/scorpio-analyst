#![cfg(feature = "test-helpers")]

#[path = "workflow_observability_collectors.rs"]
mod workflow_observability_collectors;

pub use workflow_observability_collectors::{EventCollector, StructuredEventCollector};

use std::sync::Arc;

use graph_flow::{Context, Task};
use scorpio_analyst::{
    state::{FundamentalData, NewsData, SentimentData, TechnicalData, TradingState},
    workflow::{
        SnapshotStore,
        test_support::{
            AnalystSyncTask, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
            KEY_RISK_ROUND, serialize_state_to_context, write_prefixed_result,
            write_round_debate_usage, write_round_risk_usage,
        },
    },
};
use tempfile::tempdir;
use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;

async fn make_store() -> (Arc<SnapshotStore>, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store"),
    );
    (store, dir)
}

async fn seed_all_analysts_ok(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("serialize");

    for analyst in ["fundamental", "sentiment", "news", "technical"] {
        ctx.set(format!("analyst.{analyst}.ok"), true).await;
    }

    write_prefixed_result(
        ctx,
        "analyst",
        "fundamental",
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(24.5),
            eps: Some(6.05),
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .expect("write fundamental result");

    write_prefixed_result(
        ctx,
        "analyst",
        "sentiment",
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .expect("write sentiment result");

    write_prefixed_result(
        ctx,
        "analyst",
        "news",
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .expect("write news result");

    write_prefixed_result(
        ctx,
        "analyst",
        "technical",
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
    .expect("write technical result");
}

pub fn run_analyst_sync_under_collector(collector: EventCollector) {
    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();

            seed_all_analysts_ok(&ctx, &state).await;

            let task = AnalystSyncTask::new(store);
            task.run(ctx).await.expect("AnalystSyncTask should succeed");
        });
    });
}

pub fn run_analyst_sync_under_structured_collector(collector: StructuredEventCollector) {
    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();

            seed_all_analysts_ok(&ctx, &state).await;

            let task = AnalystSyncTask::new(store);
            task.run(ctx).await.expect("AnalystSyncTask should succeed");
        });
    });
}

pub fn run_debate_accounting_under_collector(
    collector: EventCollector,
    max_rounds: u32,
    current_round: u32,
) {
    use scorpio_analyst::{
        state::AgentTokenUsage, workflow::test_support::run_debate_moderator_accounting,
    };

    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();
            serialize_state_to_context(&state, &ctx)
                .await
                .expect("serialize");

            ctx.set(KEY_MAX_DEBATE_ROUNDS, max_rounds).await;
            ctx.set(KEY_DEBATE_ROUND, current_round).await;

            if max_rounds > 0 {
                let round_number = current_round + 1;
                let bull = AgentTokenUsage::unavailable("Bullish Researcher", "stub", 1);
                let bear = AgentTokenUsage::unavailable("Bearish Researcher", "stub", 1);
                write_round_debate_usage(&ctx, round_number, &bull, &bear).await;
            }

            let mod_usage = AgentTokenUsage::unavailable("Debate Moderator", "stub", 1);
            run_debate_moderator_accounting(&ctx, &mod_usage, store).await;
        });
    });
}

pub fn run_debate_accounting_under_structured_collector(
    collector: StructuredEventCollector,
    max_rounds: u32,
    current_round: u32,
) {
    use scorpio_analyst::{
        state::AgentTokenUsage, workflow::test_support::run_debate_moderator_accounting,
    };

    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();
            serialize_state_to_context(&state, &ctx)
                .await
                .expect("serialize");

            ctx.set(KEY_MAX_DEBATE_ROUNDS, max_rounds).await;
            ctx.set(KEY_DEBATE_ROUND, current_round).await;

            if max_rounds > 0 {
                let round_number = current_round + 1;
                let bull = AgentTokenUsage::unavailable("Bullish Researcher", "stub", 1);
                let bear = AgentTokenUsage::unavailable("Bearish Researcher", "stub", 1);
                write_round_debate_usage(&ctx, round_number, &bull, &bear).await;
            }

            let mod_usage = AgentTokenUsage::unavailable("Debate Moderator", "stub", 1);
            run_debate_moderator_accounting(&ctx, &mod_usage, store).await;
        });
    });
}

pub fn run_risk_accounting_under_collector(
    collector: EventCollector,
    max_rounds: u32,
    current_round: u32,
) {
    use scorpio_analyst::{
        state::AgentTokenUsage, workflow::test_support::run_risk_moderator_accounting,
    };

    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();
            serialize_state_to_context(&state, &ctx)
                .await
                .expect("serialize");

            ctx.set(KEY_MAX_RISK_ROUNDS, max_rounds).await;
            ctx.set(KEY_RISK_ROUND, current_round).await;

            if max_rounds > 0 {
                let round_number = current_round + 1;
                let agg = AgentTokenUsage::unavailable("Aggressive Risk", "stub", 1);
                let con = AgentTokenUsage::unavailable("Conservative Risk", "stub", 1);
                let neu = AgentTokenUsage::unavailable("Neutral Risk", "stub", 1);
                write_round_risk_usage(&ctx, round_number, &agg, &con, &neu).await;
            }

            let mod_usage = AgentTokenUsage::unavailable("Risk Moderator", "stub", 1);
            run_risk_moderator_accounting(&ctx, &mod_usage, store).await;
        });
    });
}
