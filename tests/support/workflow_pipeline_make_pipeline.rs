#![cfg(feature = "test-helpers")]

use std::sync::Arc;

use scorpio_analyst::{
    config::{ApiConfig, Config, LlmConfig, TradingConfig},
    data::{FinnhubClient, FredClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    rate_limit::SharedRateLimiter,
    workflow::{SnapshotStore, TradingPipeline},
};
use tempfile::tempdir;

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
            ..ApiConfig::default()
        },
        storage: Default::default(),
        rate_limits: Default::default(),
    };

    let finnhub = FinnhubClient::for_test();
    let fred = FredClient::for_test();
    let yfinance = YFinanceClient::new(SharedRateLimiter::new(limiter_name, 10));
    let handle = CompletionModelHandle::for_test();

    let pipeline = TradingPipeline::new(
        config,
        finnhub,
        fred,
        yfinance,
        pipeline_store,
        handle.clone(),
        handle,
    );

    (pipeline, verify_store, dir)
}
