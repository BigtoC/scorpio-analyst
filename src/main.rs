use chrono::Local;
use figlet_rs::Toilet;
use scorpio_analyst::config::Config;
use scorpio_analyst::data::{FinnhubClient, FredClient, YFinanceClient};
use scorpio_analyst::observability::init_tracing;
use scorpio_analyst::providers::ModelTier;
use scorpio_analyst::providers::factory::{
    create_completion_model, preflight_copilot_if_configured,
};
use scorpio_analyst::rate_limit::{ProviderRateLimiters, SharedRateLimiter};
use scorpio_analyst::state::TradingState;
use scorpio_analyst::workflow::{SnapshotStore, TradingPipeline};

fn main() {
    init_tracing();

    if let Ok(font) = Toilet::mono12()
        && let Some(figure) = font.convert("Scorpio Analyst")
    {
        println!("{}", figure.as_str());
    }

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

            if let Err(e) = runtime.block_on(preflight_copilot_if_configured(
                &cfg.llm,
                &cfg.providers,
                &ProviderRateLimiters::from_config(&cfg.providers),
            )) {
                eprintln!("failed to preflight configured Copilot provider: {e:#}");
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

            let rate_limiters = ProviderRateLimiters::from_config(&cfg.providers);

            let quick_handle = match create_completion_model(
                ModelTier::QuickThinking,
                &cfg.llm,
                &cfg.providers,
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
                &cfg.providers,
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
            let fred_limiter = SharedRateLimiter::fred_from_config(&cfg.rate_limits)
                .unwrap_or_else(|| SharedRateLimiter::disabled("fred"));
            let fred = match FredClient::new(&cfg.api, fred_limiter) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("failed to initialize FRED client: {e:#}");
                    std::process::exit(1);
                }
            };
            let yfinance = YFinanceClient::from_config(&cfg.rate_limits);

            // TODO(Unit-6): symbol will be provided as a CLI argument by `scorpio analyze`.
            // Temporarily read from env var so the binary stays runnable during transition.
            let symbol =
                std::env::var("SCORPIO_ASSET_SYMBOL").unwrap_or_else(|_| "AAPL".to_owned());
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
                fred,
                yfinance,
                snapshot_store,
                quick_handle,
                deep_handle,
            );

            let initial_state = TradingState::new(&symbol, &target_date);

            match runtime.block_on(pipeline.run_analysis_cycle(initial_state)) {
                Ok(state) => {
                    if state.final_execution_status.is_none() {
                        eprintln!("pipeline completed without a final execution status");
                        std::process::exit(1);
                    }
                    println!("{}", scorpio_analyst::report::format_final_report(&state));
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
