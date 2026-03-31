use chrono::Local;
use scorpio_analyst::config::Config;
use scorpio_analyst::data::{FinnhubClient, YFinanceClient};
use scorpio_analyst::observability::init_tracing;
use scorpio_analyst::providers::ModelTier;
use scorpio_analyst::providers::factory::{
    create_completion_model, preflight_configured_providers,
};
use scorpio_analyst::rate_limit::{ProviderRateLimiters, SharedRateLimiter};
use scorpio_analyst::state::TradingState;
use scorpio_analyst::workflow::{SnapshotStore, TradingPipeline};

fn main() {
    init_tracing();

    match Config::load() {
        Ok(cfg) => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("failed to initialize async runtime: {e:#}");
                    std::process::exit(1);
                }
            };

            if let Err(e) = runtime.block_on(preflight_configured_providers(
                &cfg.llm,
                &cfg.api,
                &ProviderRateLimiters::from_config(&cfg.rate_limits),
            )) {
                eprintln!("failed to preflight configured providers: {e:#}");
                std::process::exit(1);
            }

            let snapshot_store = match runtime.block_on(SnapshotStore::from_config(&cfg)) {
                Ok(store) => store,
                Err(e) => {
                    eprintln!("failed to initialize snapshot storage: {e:#}");
                    std::process::exit(1);
                }
            };

            tracing::info!(snapshot_store = ?snapshot_store, "storage configured");

            let rate_limiters = ProviderRateLimiters::from_config(&cfg.rate_limits);

            let quick_handle = match create_completion_model(
                ModelTier::QuickThinking,
                &cfg.llm,
                &cfg.api,
                &rate_limiters,
            ) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("failed to create quick-thinking model handle: {e:#}");
                    std::process::exit(1);
                }
            };

            let deep_handle = match create_completion_model(
                ModelTier::DeepThinking,
                &cfg.llm,
                &cfg.api,
                &rate_limiters,
            ) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("failed to create deep-thinking model handle: {e:#}");
                    std::process::exit(1);
                }
            };

            let finnhub_limiter = SharedRateLimiter::finnhub_from_config(&cfg.rate_limits)
                .unwrap_or_else(|| SharedRateLimiter::disabled("finnhub"));
            let finnhub = match FinnhubClient::new(&cfg.api, finnhub_limiter) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("failed to initialize Finnhub client: {e:#}");
                    std::process::exit(1);
                }
            };
            let yfinance = YFinanceClient::default();

            let symbol = cfg.trading.asset_symbol.clone();
            let target_date = Local::now().format("%Y-%m-%d").to_string();

            tracing::info!(
                quick_provider = %cfg.llm.quick_thinking_provider,
                deep_provider = %cfg.llm.deep_thinking_provider,
                symbol = %symbol,
                target_date = %target_date,
                "scorpio-analyst initialized"
            );

            let pipeline = TradingPipeline::new(
                cfg,
                finnhub,
                yfinance,
                snapshot_store,
                quick_handle,
                deep_handle,
            );

            let initial_state = TradingState::new(&symbol, &target_date);

            match runtime.block_on(pipeline.run_analysis_cycle(initial_state)) {
                Ok(state) => {
                    match &state.final_execution_status {
                        Some(execution) => {
                            println!(
                                "\n=== DECISION: {:?} ===\n{}\n",
                                execution.decision, execution.rationale
                            );
                        }
                        None => {
                            eprintln!("pipeline completed without a final execution status");
                            std::process::exit(1);
                        }
                    }
                    println!(
                        "Token usage: {} total ({} prompt / {} completion)",
                        state.token_usage.total_tokens,
                        state.token_usage.total_prompt_tokens,
                        state.token_usage.total_completion_tokens,
                    );
                }
                Err(e) => {
                    eprintln!("analysis cycle failed: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("failed to load configuration: {e:#}");
            std::process::exit(1);
        }
    }
}
