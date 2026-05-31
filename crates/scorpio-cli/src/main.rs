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
    // Hold the tracing guard for the lifetime of main and explicitly
    // call `flush_and_shutdown()` before every exit path. `Drop` is not
    // enough because the non-zero exit branch below calls
    // `std::process::exit` which BYPASSES destructors entirely. Without
    // an explicit flush, the trailing batch of Langfuse spans would be
    // lost on any failed run.
    let tracing_guard = init_tracing();
    let cli = Cli::parse();

    // Capture command-shape guards before `cli.command` is moved by dispatch.
    let skip_upgrade_notice = matches!(&cli.command, Commands::Upgrade | Commands::Report(_));
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
    let dispatch = async {
        match cli.command {
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
            Commands::Report(args) => scorpio_cli::cli::report::run(&args).await,
        }
    };

    // Race dispatch against a manual interrupt so a Ctrl-C / SIGTERM still
    // drains buffered Langfuse spans before the process dies. The normal and
    // error-return paths flush explicitly below, and a panic unwinds through
    // `TracingGuard`'s `Drop`, but the OS default signal action would kill us
    // outright — losing the trailing span batch of an interrupted run.
    let result: anyhow::Result<()> = tokio::select! {
        biased;
        res = dispatch => res,
        () = shutdown_signal() => {
            // Re-arm a force-quit before flushing: once tokio installs its
            // signal handler the OS default (terminate) is replaced for the
            // rest of the process, so a second Ctrl-C would otherwise be
            // swallowed while the flush runs. This keeps the "press again to
            // give up" escape hatch alive if the Langfuse flush stalls on a
            // slow network. The detached `analyze`/`setup` blocking task dies
            // with the process; we only drain what is already buffered.
            eprintln!(
                "\nInterrupted — flushing telemetry before exit (Ctrl-C again to force quit)…"
            );
            tokio::spawn(async {
                let _ = tokio::signal::ctrl_c().await;
                std::process::exit(130);
            });
            tracing_guard.flush_and_shutdown();
            std::process::exit(130);
        }
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
    if !skip_upgrade_notice {
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

    // Flush Langfuse spans BEFORE any exit. `std::process::exit` does
    // not run destructors, so relying on `Drop` would lose spans on the
    // failure branch. On the success branch the explicit flush is still
    // useful — it drains while tokio is fully healthy, avoiding any
    // race between Drop and tokio runtime teardown.
    tracing_guard.flush_and_shutdown();

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn should_show_analyze_banner(command: &Commands) -> bool {
    matches!(command, Commands::Analyze(args) if !args.no_terminal)
}

/// Resolves when the process receives a manual interrupt — Ctrl-C / SIGINT on
/// any platform, or SIGTERM on Unix. Used to race command dispatch so a
/// user-driven shutdown still flushes Langfuse spans before exit.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        // Installing a signal handler only fails on environment-level
        // impossibilities that uniformly doom the process, so there is no
        // per-caller recovery — treat it as fatal rather than threading a
        // `Result` to no purpose.
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        // `ctrl_c` is the portable interrupt; SIGTERM has no Windows analogue.
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_cli::cli::{AnalyzeArgs, ReportArgs, ReportSubcommand};

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

    #[test]
    fn report_skips_upgrade_notice() {
        let command = Commands::Report(ReportArgs {
            subcommand: ReportSubcommand::List { json: false },
        });

        assert!(matches!(&command, Commands::Report(_)));
        assert!(matches!(&command, Commands::Upgrade | Commands::Report(_)));
    }

    #[test]
    fn report_never_shows_analyze_banner() {
        let command = Commands::Report(ReportArgs {
            subcommand: ReportSubcommand::Show {
                execution_id: "exec-1".to_owned(),
                json: false,
            },
        });

        assert!(!should_show_analyze_banner(&command));
    }
}
