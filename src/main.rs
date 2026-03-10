use scorpio_analyst::config::Config;
use scorpio_analyst::observability::init_tracing;

fn main() {
    init_tracing();

    match Config::load() {
        Ok(cfg) => {
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
