use std::io::{self, IsTerminal};

use clap::Parser;
use scorpio_analyst::cli::update::{
    check_latest_version, run_upgrade, try_show_update_notice_with_tty,
};
use scorpio_analyst::cli::{Cli, Commands};
use scorpio_analyst::observability::init_tracing;

/// Current scorpio version, embedded at build time from `Cargo.toml`.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    // Capture upgrade-guard before `cli.command` is moved by the dispatch match.
    let is_upgrade = matches!(cli.command, Commands::Upgrade);

    // Background update check (non-blocking, fire-and-forget). Gated by the
    // `--no-update-check` flag / `SCORPIO_NO_UPDATE_CHECK` env var.
    let update_rx = if cli.no_update_check {
        None
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let latest = check_latest_version().await;
            let _ = tx.send(latest);
        });
        Some(rx)
    };

    // Dispatch. Existing synchronous subcommands build their own tokio
    // runtime internally; calling them from async context would panic, so
    // we bridge via `spawn_blocking`. `Upgrade` is natively async.
    let result: anyhow::Result<()> = match cli.command {
        Commands::Analyze { symbol } => {
            tokio::task::spawn_blocking(move || scorpio_analyst::cli::analyze::run(&symbol))
                .await
                .map_err(|e| anyhow::anyhow!("analyze task failed to join: {e}"))
                .and_then(|r| r)
        }
        Commands::Setup => tokio::task::spawn_blocking(scorpio_analyst::cli::setup::run)
            .await
            .map_err(|e| anyhow::anyhow!("setup task failed to join: {e}"))
            .and_then(|r| r),
        Commands::Upgrade => run_upgrade().await,
    };

    if let Err(e) = result {
        eprintln!("{e:#}");
        std::process::exit(1);
    }

    // Post-command notice. Skip for `Upgrade` so we don't tell the user to
    // run `scorpio upgrade` immediately after they just did.
    if !is_upgrade
        && let Some(rx) = update_rx
        && let Some(notice) =
            try_show_update_notice_with_tty(rx, CURRENT_VERSION, io::stderr().is_terminal())
    {
        eprintln!("{notice}");
    }
}
