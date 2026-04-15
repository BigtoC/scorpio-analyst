use clap::Parser;
use scorpio_analyst::cli::{Cli, Commands};
use scorpio_analyst::observability::init_tracing;

fn main() {
    init_tracing();
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Analyze { ref symbol } => scorpio_analyst::cli::analyze::run(symbol),
        Commands::Setup => scorpio_analyst::cli::setup::run(),
    };
    if let Err(e) = result {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
