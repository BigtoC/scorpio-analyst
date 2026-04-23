use std::io::{self, IsTerminal};
use std::time::Duration;

use clap::Parser;
use scorpio_cli::cli::update::{
    NoticeOutcome, check_latest_version, run_upgrade, show_update_notice_with_tty,
};
use scorpio_cli::cli::{Cli, Commands};
use scorpio_core::observability::init_tracing;

/// Current scorpio version, embedded at build time from `Cargo.toml`.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Post-command grace window for the background update check. Small enough
/// that users don't feel lag when the check has genuinely failed, large enough
/// to catch typical GitHub API responses on fast-exiting subcommands.
const UPDATE_NOTICE_GRACE: Duration = Duration::from_millis(500);

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    // Capture command-shape guards before `cli.command` is moved by dispatch.
    let is_upgrade = matches!(&cli.command, Commands::Upgrade);
    let show_banner = should_show_analyze_banner(&cli.command);

    // Background update check (non-blocking, fire-and-forget). Gated by the
    // `--no-update-check` flag / `SCORPIO_NO_UPDATE_CHECK` env var.
    let mut update_rx = if cli.no_update_check {
        None
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let latest = check_latest_version().await;
            let _ = tx.send(latest);
        });
        Some(rx)
    };

    // For `analyze`, show the banner + update notice BEFORE the minutes-long
    // pipeline starts so the user can Ctrl-C and upgrade first if they want.
    // The notice is also cached so it can be re-printed after the final
    // report, giving the user a second reminder once the run completes.
    // If the grace window expires before the background check finishes (cold
    // DNS / slow network), the receiver is returned to `update_rx` so the
    // post-command block below gets a second chance.
    let mut cached_notice: Option<String> = None;
    if show_banner {
        scorpio_cli::cli::analyze::print_banner();
        if let Some(rx) = update_rx.take() {
            match show_update_notice_with_tty(
                rx,
                CURRENT_VERSION,
                io::stderr().is_terminal(),
                UPDATE_NOTICE_GRACE,
            )
            .await
            {
                NoticeOutcome::Ready(notice) => {
                    eprintln!("{notice}");
                    cached_notice = Some(notice);
                }
                NoticeOutcome::Resolved => {}
                NoticeOutcome::Pending(rx) => update_rx = Some(rx),
            }
        }
    }

    // Dispatch. Existing synchronous subcommands build their own tokio
    // runtime internally; calling them from async context would panic, so
    // we bridge via `spawn_blocking`. `Upgrade` is natively async.
    let result: anyhow::Result<()> = match cli.command {
        Commands::Analyze(args) => {
            let args = args.clone();
            tokio::task::spawn_blocking(move || scorpio_cli::cli::analyze::run(&args))
                .await
                .map_err(|e| anyhow::anyhow!("analyze task failed to join: {e}"))
                .and_then(|r| r)
        }
        Commands::Setup => tokio::task::spawn_blocking(scorpio_cli::cli::setup::run)
            .await
            .map_err(|e| anyhow::anyhow!("setup task failed to join: {e}"))
            .and_then(|r| r),
        Commands::Upgrade => run_upgrade().await,
    };

    let exit_code = if let Err(e) = result {
        eprintln!("{e:#}");
        1
    } else {
        0
    };

    // Post-command notice. Three cases:
    //   - `analyze` hit pre-dispatch: replay the cached notice after the
    //     final report so users see it at both ends of a long run.
    //   - `analyze` whose pre-dispatch timed out: retry now that the pipeline
    //     has had minutes to run.
    //   - `setup`: the normal post-dispatch path.
    // `upgrade` is skipped so we don't tell the user to run `scorpio upgrade`
    // immediately after they just did. Rendered even on the error path so
    // users notice a stale binary regardless of whether the subcommand
    // succeeded.
    if !is_upgrade {
        let end_notice = if let Some(n) = cached_notice {
            Some(n)
        } else if let Some(rx) = update_rx {
            match show_update_notice_with_tty(
                rx,
                CURRENT_VERSION,
                io::stderr().is_terminal(),
                UPDATE_NOTICE_GRACE,
            )
            .await
            {
                NoticeOutcome::Ready(n) => Some(n),
                NoticeOutcome::Resolved | NoticeOutcome::Pending(_) => None,
            }
        } else {
            None
        };
        if let Some(notice) = end_notice {
            eprintln!("{notice}");
        }
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn should_show_analyze_banner(command: &Commands) -> bool {
    matches!(command, Commands::Analyze(args) if !args.no_terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_cli::cli::AnalyzeArgs;

    #[test]
    fn analyze_shows_banner_when_terminal_output_is_enabled() {
        assert!(should_show_analyze_banner(&Commands::Analyze(
            AnalyzeArgs {
                symbol: "AAPL".to_owned(),
                ..Default::default()
            }
        )));
    }

    #[test]
    fn analyze_skips_banner_when_no_terminal_is_requested() {
        assert!(!should_show_analyze_banner(&Commands::Analyze(
            AnalyzeArgs {
                symbol: "AAPL".to_owned(),
                no_terminal: true,
                json: true,
                ..Default::default()
            }
        )));
    }
}
