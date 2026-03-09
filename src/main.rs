use scorpio_analyst::config::Config;
use scorpio_analyst::observability::init_tracing;

fn main() {
    init_tracing();

    match Config::load() {
        Ok(cfg) => {
            tracing::info!(
                provider = %cfg.llm.default_provider,
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
