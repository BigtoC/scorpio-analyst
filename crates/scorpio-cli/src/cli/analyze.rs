//! `scorpio analyze <SYMBOL>` subcommand handler.
//!
//! Loads the runtime config, validates the symbol up-front for fail-fast UX,
//! builds a CLI-owned tokio runtime, and hands the heavy lifting off to
//! [`AnalysisRuntime`]. Presentation (figlet banner and final-report
//! formatting) stays in this crate; assembly + pipeline execution live in
//! `scorpio-core`.

use anyhow::Context;
use figlet_rs::Toilet;

use scorpio_core::app::AnalysisRuntime;
use scorpio_core::config::Config;
use scorpio_core::data::symbol::validate_symbol;

/// Error message printed when the user config is missing or incomplete.
const CONFIG_MISSING_MSG: &str = "✗ Config not found or incomplete. Run `scorpio setup` to configure your API keys and providers.";

/// Print the "Scorpio Analyst" figlet banner to stdout.
///
/// Lifted out of [`run`] so `main.rs` can render it before the post-banner
/// update notice, letting users Ctrl-C and upgrade before the minutes-long
/// pipeline starts.
pub fn print_banner() {
    if let Ok(font) = Toilet::mono12()
        && let Some(figure) = font.convert("Scorpio Analyst")
    {
        println!("{}", figure.as_str());
    }
}

/// Run the full 5-phase analysis pipeline for `symbol`.
///
/// Synchronous shell around the async [`AnalysisRuntime`]: builds a CLI-owned
/// current-thread tokio runtime (matching the pre-facade shape), validates the
/// symbol for fail-fast UX, assembles the runtime, executes one cycle, and
/// prints the formatted report to stdout.
///
/// # Errors
///
/// Returns `Err` for:
/// - missing or incomplete user config
/// - invalid symbol format
/// - any facade assembly or pipeline runtime failure
pub fn run(symbol: &str) -> anyhow::Result<()> {
    let cfg = load_analysis_config()?;

    // Early validation preserves today's fail-fast UX before any runtime
    // assembly starts. The facade re-validates defensively so non-CLI
    // consumers get the same input contract.
    let _ = validate_symbol(symbol)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async runtime")?;

    runtime.block_on(async move {
        let analysis = AnalysisRuntime::new(cfg).await?;
        let state = analysis.run(symbol).await?;
        println!("{}", crate::report::format_final_report(&state));
        Ok::<(), anyhow::Error>(())
    })
}

fn load_analysis_config() -> anyhow::Result<Config> {
    let cfg = Config::load().map_err(|e| e.context(CONFIG_MISSING_MSG))?;
    cfg.is_analysis_ready().context(CONFIG_MISSING_MSG)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises tests that override HOME / env vars.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── Missing-config guard ──────────────────────────────────────────────────

    #[test]
    fn run_missing_config_returns_config_not_found_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: serialised by ENV_LOCK
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run("AAPL");
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

        let result = run("AAPL");

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

    // ── Symbol validation (re-homed from Config::validate() in Unit 3) ───────

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
        let result = run("");
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
        let result = run("DROP;TABLE");
        unsafe { std::env::remove_var("HOME") };
        let err = result.expect_err("symbol with semicolons should return Err");
        assert!(
            err.to_string().contains("invalid symbol"),
            "error should mention 'invalid symbol'; got: {err}"
        );
    }

    #[test]
    fn run_accepts_lowercase_symbol_past_validation() {
        // Only tests that the symbol validator doesn't reject lowercase —
        // pipeline failure (no real LLM/data) is expected and acceptable.
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        write_minimal_config(&dir);
        unsafe { std::env::set_var("HOME", dir.path()) };
        let result = run("nvda");
        unsafe { std::env::remove_var("HOME") };
        // Must NOT be a "Config not found" or "invalid symbol" error
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
}
