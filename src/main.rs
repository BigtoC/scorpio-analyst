use scorpio_analyst::config::Config;
use scorpio_analyst::observability::init_tracing;
use scorpio_analyst::providers::factory::preflight_configured_providers;
use scorpio_analyst::workflow::SnapshotStore;

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

            if let Err(e) = runtime.block_on(preflight_configured_providers(&cfg.llm, &cfg.api)) {
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

            tracing::info!(
                snapshot_store = ?snapshot_store,
                "storage configured"
            );

            tracing::info!(
                quick_provider = %cfg.llm.quick_thinking_provider,
                deep_provider = %cfg.llm.deep_thinking_provider,
                symbol = %cfg.trading.asset_symbol,
                "scorpio-analyst initialized"
            );
        }
        Err(e) => {
            eprintln!("failed to load configuration: {e:#}");
            std::process::exit(1);
        }
    }
}
