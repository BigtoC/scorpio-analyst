#![cfg(feature = "test-helpers")]
#![allow(dead_code)]

use std::sync::Arc;

use scorpio_analyst::{
    config::{ApiConfig, Config, LlmConfig, TradingConfig},
    data::{FinnhubClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    rate_limit::SharedRateLimiter,
    state::TradingState,
    workflow::{SnapshotPhase, SnapshotStore, TradingPipeline},
};
use tempfile::tempdir;

pub fn phase_from_number(phase: u8) -> SnapshotPhase {
    match phase {
        1 => SnapshotPhase::AnalystTeam,
        2 => SnapshotPhase::ResearcherDebate,
        3 => SnapshotPhase::Trader,
        4 => SnapshotPhase::RiskDiscussion,
        5 => SnapshotPhase::FundManager,
        _ => panic!("unsupported snapshot phase: {phase}"),
    }
}

pub async fn make_pipeline(
    db_name: &str,
    limiter_name: &str,
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> (TradingPipeline, Arc<SnapshotStore>, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join(db_name);

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
    let yfinance = YFinanceClient::new(SharedRateLimiter::new(limiter_name, 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    (pipeline, verify_store, dir)
}

pub async fn run_stubbed_pipeline(
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> (TradingState, Arc<SnapshotStore>, tempfile::TempDir) {
    let (pipeline, verify_store, dir) = make_pipeline(
        "e2e-test.db",
        "e2e-test",
        max_debate_rounds,
        max_risk_rounds,
    )
    .await;

    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let initial_state = TradingState::new("AAPL", "2026-03-20");
    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must complete successfully with stubs");

    (final_state, verify_store, dir)
}
