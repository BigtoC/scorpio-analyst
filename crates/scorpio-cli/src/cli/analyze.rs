//! `scorpio analyze <SYMBOL>` subcommand handler.
//!
//! Loads the runtime config, validates the symbol up-front for fail-fast UX,
//! builds a multi-thread tokio runtime so spawned reporter tasks run in
//! genuine parallel, and hands the heavy lifting off to [`AnalysisRuntime`].
//! The reporter chain is assembled from CLI flags and executed concurrently
//! via [`ReporterChain::run_all`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::Utc;
use figlet_rs::Toilet;

use scorpio_core::app::AnalysisRuntime;
use scorpio_core::config::Config;
use scorpio_core::data::symbol::validate_symbol;
use scorpio_reporters::json::JsonReporter;
use scorpio_reporters::terminal::TerminalReporter;
use scorpio_reporters::{ReportContext, ReporterChain};

use super::AnalyzeArgs;

const CONFIG_MISSING_MSG: &str = "✗ Config not found or incomplete. Run `scorpio setup` to configure your API keys and providers.";

/// Print the "Scorpio Analyst" figlet banner to stdout.
///
/// Gated on `!args.no_terminal` in `main.rs` before the pipeline starts so
/// users can Ctrl-C and upgrade before the minutes-long run begins.
pub fn print_banner() {
    if let Ok(font) = Toilet::mono12()
        && let Some(figure) = font.convert("Scorpio Analyst")
    {
        println!("{}", figure.as_str());
    }
}

/// Run the full 5-phase analysis pipeline and emit results through all
/// configured reporters.
///
/// Synchronous entry point (called from `spawn_blocking`). Builds a
/// `new_multi_thread` runtime so reporter tasks spawned by
/// [`ReporterChain::run_all`] run on separate OS threads.
///
/// # Errors
///
/// Returns `Err` for:
/// - missing or incomplete user config
/// - invalid symbol format
/// - no reporters enabled (`--no-terminal` without any other reporter)
/// - every requested reporter failing after analysis completes
/// - any facade assembly or pipeline runtime failure
pub fn run(args: &AnalyzeArgs) -> anyhow::Result<()> {
    validate_reporter_args(args)?;
    let cfg = load_analysis_config()?;
    let _ = validate_symbol(&args.symbol)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to initialize async runtime")?;

    runtime.block_on(async move {
        let chain = build_reporter_chain(args);
        let n = chain.len();
        anyhow::ensure!(
            n > 0,
            "at least one reporter must be enabled; use --json if --no-terminal is set"
        );

        let analysis = AnalysisRuntime::new(cfg).await?;
        let state = Arc::new(analysis.run(&args.symbol).await?);
        let ctx = Arc::new(ReportContext {
            symbol: state.asset_symbol.clone(),
            finished_at: Utc::now(),
            output_dir: report_output_dir(args)?,
        });

        let failures = chain.run_all(state, ctx).await;
        if failures == n {
            anyhow::bail!("{failures} reporter(s) failed; see logs");
        }
        Ok(())
    })
}

fn build_reporter_chain(args: &AnalyzeArgs) -> ReporterChain {
    let mut chain = ReporterChain::new();
    if !args.no_terminal {
        chain.push(TerminalReporter);
    }
    if args.json {
        chain.push(JsonReporter);
    }
    chain
}

fn validate_reporter_args(args: &AnalyzeArgs) -> anyhow::Result<()> {
    if args.no_terminal && !args.json {
        anyhow::bail!("at least one reporter must be enabled; use --json if --no-terminal is set");
    }

    if args.output_dir.is_some() && !args.json {
        anyhow::bail!("--output-dir requires --json");
    }

    Ok(())
}

fn report_output_dir(args: &AnalyzeArgs) -> anyhow::Result<Option<PathBuf>> {
    if !args.json {
        return Ok(None);
    }

    resolve_reports_dir(args.output_dir.as_deref()).map(Some)
}

/// Resolve the output directory for file reporters.
///
/// If `--output-dir` is provided, use it as-is. Otherwise default to
/// `$HOME/.scorpio-analyst/reports`, matching the app-owned path style used
/// for phase snapshots.
fn resolve_reports_dir(output_dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(dir) = output_dir {
        return Ok(dir.to_path_buf());
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or_else(|| anyhow::anyhow!("HOME environment variable is not set"))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        anyhow::bail!("HOME must be an absolute path; got: {}", home.display());
    }
    Ok(home.join(".scorpio-analyst/reports"))
}

fn load_analysis_config() -> anyhow::Result<Config> {
    let cfg = Config::load().map_err(|e| e.context(CONFIG_MISSING_MSG))?;
    cfg.is_analysis_ready().context(CONFIG_MISSING_MSG)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn args_for(symbol: &str, dir: &tempfile::TempDir) -> AnalyzeArgs {
        AnalyzeArgs {
            symbol: symbol.to_owned(),
            json: true,
            output_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        }
    }

    // ── Missing-config guard ──────────────────────────────────────────────────

    #[test]
    fn run_missing_config_returns_config_not_found_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run(&args_for("AAPL", &dir));
        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("missing config should return Err");
        assert!(
            err.to_string().contains("Config not found or incomplete"),
            "error should mention 'Config not found or incomplete'; got: {err}"
        );
    }

    #[test]
    fn run_env_only_config_does_not_fail_with_config_not_found() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER", "openai");
            std::env::set_var("SCORPIO__LLM__DEEP_THINKING_PROVIDER", "openai");
            std::env::set_var("SCORPIO__LLM__QUICK_THINKING_MODEL", "gpt-4o-mini");
            std::env::set_var("SCORPIO__LLM__DEEP_THINKING_MODEL", "o3");
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "sk-env-test");
            std::env::set_var("SCORPIO_FINNHUB_API_KEY", "fh-env-test");
            std::env::set_var("SCORPIO_FRED_API_KEY", "fred-env-test");
        }

        let result = run(&args_for("AAPL", &dir));

        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER");
            std::env::remove_var("SCORPIO__LLM__DEEP_THINKING_PROVIDER");
            std::env::remove_var("SCORPIO__LLM__QUICK_THINKING_MODEL");
            std::env::remove_var("SCORPIO__LLM__DEEP_THINKING_MODEL");
            std::env::remove_var("SCORPIO_OPENAI_API_KEY");
            std::env::remove_var("SCORPIO_FINNHUB_API_KEY");
            std::env::remove_var("SCORPIO_FRED_API_KEY");
        }

        match result {
            Ok(()) => {}
            Err(err) => assert!(
                !err.to_string().contains("Config not found or incomplete"),
                "env-only config should get past config-not-found gate; got: {err}"
            ),
        }
    }

    // ── Symbol validation ─────────────────────────────────────────────────────

    fn write_minimal_config(dir: &tempfile::TempDir) {
        use scorpio_core::settings::{PartialConfig, save_user_config_at};
        let config_path = dir.path().join(".scorpio-analyst/config.toml");
        let partial = PartialConfig {
            finnhub_api_key: Some("fh-test".into()),
            fred_api_key: Some("fred-test".into()),
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-test".into()),
            ..Default::default()
        };
        save_user_config_at(&partial, &config_path).unwrap();
    }

    #[test]
    fn run_rejects_empty_symbol() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_minimal_config(&dir);
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run(&AnalyzeArgs {
            symbol: "".to_owned(),
            json: true,
            output_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        });
        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("empty symbol should return Err");
        assert!(
            err.to_string().contains("invalid symbol"),
            "error should mention 'invalid symbol'; got: {err}"
        );
    }

    #[test]
    fn run_rejects_symbol_with_semicolons() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_minimal_config(&dir);
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run(&AnalyzeArgs {
            symbol: "DROP;TABLE".to_owned(),
            json: true,
            output_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        });
        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("symbol with semicolons should return Err");
        assert!(
            err.to_string().contains("invalid symbol"),
            "error should mention 'invalid symbol'; got: {err}"
        );
    }

    #[test]
    fn run_accepts_lowercase_symbol_past_validation() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_minimal_config(&dir);
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run(&args_for("nvda", &dir));
        unsafe { std::env::remove_var("HOME") };
        match result {
            Ok(()) => {}
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("Config not found or incomplete"),
                    "lowercase symbol should pass config check; got: {msg}"
                );
                assert!(
                    !msg.contains("invalid symbol"),
                    "lowercase symbol should pass validation; got: {msg}"
                );
            }
        }
    }

    // ── Reporter chain validation ──────────────────────────────────────────────

    #[test]
    fn run_rejects_no_terminal_without_any_other_reporter() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_minimal_config(&dir);
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run(&AnalyzeArgs {
            symbol: "AAPL".to_owned(),
            no_terminal: true,
            json: false,
            output_dir: Some(dir.path().to_path_buf()),
        });
        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("--no-terminal alone should be rejected");
        assert!(
            err.to_string().contains("at least one reporter"),
            "error should explain that a reporter is required; got: {err}"
        );
    }

    #[test]
    fn validate_reporter_args_rejects_output_dir_without_json() {
        let err = validate_reporter_args(&AnalyzeArgs {
            symbol: "AAPL".to_owned(),
            output_dir: Some(PathBuf::from("/tmp/reports")),
            ..Default::default()
        })
        .expect_err("output_dir without a file reporter should be rejected");
        assert!(
            err.to_string().contains("--output-dir requires --json"),
            "error should explain output_dir requires json; got: {err}"
        );
    }

    #[test]
    fn report_output_dir_skips_home_lookup_when_json_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("HOME") };
        let dir = report_output_dir(&AnalyzeArgs {
            symbol: "AAPL".to_owned(),
            json: false,
            output_dir: None,
            ..Default::default()
        })
        .expect("terminal-only runs should not require HOME");
        assert_eq!(dir, None);
    }

    #[test]
    fn run_rejects_no_terminal_without_any_other_reporter_before_config_load() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };

        let result = run(&AnalyzeArgs {
            symbol: "AAPL".to_owned(),
            no_terminal: true,
            json: false,
            output_dir: None,
        });

        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("invalid reporter selection should fail before config load");
        assert!(
            err.to_string().contains("at least one reporter"),
            "expected reporter validation error, got: {err}"
        );
    }

    // ── resolve_reports_dir ────────────────────────────────────────────────────

    #[test]
    fn resolve_reports_dir_returns_provided_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_reports_dir(Some(dir.path())).unwrap();
        assert_eq!(result, dir.path());
    }

    #[test]
    fn resolve_reports_dir_defaults_to_home_scorpio_analyst_reports() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = resolve_reports_dir(None).unwrap();
        unsafe { std::env::remove_var("HOME") };
        assert_eq!(result, dir.path().join(".scorpio-analyst/reports"));
    }
}
