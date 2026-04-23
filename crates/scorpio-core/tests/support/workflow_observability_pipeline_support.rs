#![cfg(feature = "test-helpers")]

#[path = "workflow_observability_collectors.rs"]
mod workflow_observability_collectors;

pub use workflow_observability_collectors::{EventCollector, StructuredEventCollector};

use scorpio_core::{
    config::{ApiConfig, Config, LlmConfig, TradingConfig},
    data::{FinnhubClient, FredClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    rate_limit::SharedRateLimiter,
    state::TradingState,
    workflow::{SnapshotStore, TradingPipeline},
};
use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;

fn obs_test_config() -> Config {
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
        api: ApiConfig {
            ..ApiConfig::default()
        },
        storage: Default::default(),
        providers: Default::default(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
        analysis_pack: "baseline".to_owned(),
    }
}

pub fn run_stubbed_pipeline_under_collector(collector: EventCollector, db_name: &str) {
    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let db_path = dir.path().join(db_name);
            let pipeline_store = SnapshotStore::new(Some(&db_path)).await.expect("store");
            let config = obs_test_config();
            let finnhub = FinnhubClient::for_test();
            let fred = FredClient::for_test();
            let yfinance = YFinanceClient::new(SharedRateLimiter::new("obs-test", 10));
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
            pipeline
                .install_stub_tasks_for_test()
                .expect("stub install must succeed");

            let state = TradingState::new("AAPL", "2026-03-20");
            let _ = pipeline.run_analysis_cycle(state).await;
        });
    });
}

pub fn run_stubbed_pipeline_under_structured_collector(
    collector: StructuredEventCollector,
    db_name: &str,
) -> TradingState {
    let subscriber = tracing_subscriber::registry().with(collector);

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let db_path = dir.path().join(db_name);
            let pipeline_store = SnapshotStore::new(Some(&db_path)).await.expect("store");
            let config = obs_test_config();
            let finnhub = FinnhubClient::for_test();
            let fred = FredClient::for_test();
            let yfinance = YFinanceClient::new(SharedRateLimiter::new("obs-test", 10));
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
            pipeline
                .install_stub_tasks_for_test()
                .expect("stub install must succeed");

            let state = TradingState::new("AAPL", "2026-03-20");
            pipeline
                .run_analysis_cycle(state)
                .await
                .expect("stubbed pipeline should succeed")
        })
    })
}
