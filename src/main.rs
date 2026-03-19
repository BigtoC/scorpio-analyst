use scorpio_analyst::config::{Config, expand_path};
use scorpio_analyst::observability::init_tracing;
use scorpio_analyst::providers::factory::preflight_configured_providers;

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

            let snapshot_db_path = expand_path(&cfg.storage.snapshot_db_path);
            tracing::info!(
                snapshot_db_path = %snapshot_db_path.display(),
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
