//! `scorpio analyze <SYMBOL>` subcommand handler.
//!
//! Lifts the existing `main.rs` pipeline body into a testable function, adds
//! a user-facing config-not-found guard, and moves symbol validation here from
//! the now-removed `Config::validate()`.

use anyhow::Context;
use chrono::Local;
use figlet_rs::Toilet;

use crate::config::Config;
use crate::data::{FinnhubClient, FredClient, YFinanceClient};
use crate::providers::ModelTier;
use crate::providers::factory::{create_completion_model, preflight_copilot_if_configured};
use crate::rate_limit::{ProviderRateLimiters, SharedRateLimiter};
use crate::state::TradingState;
use crate::workflow::{SnapshotStore, TradingPipeline};

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
/// # Errors
///
/// Returns `Err` (with a message printed to stderr) for:
/// - missing or incomplete user config
/// - invalid symbol format
/// - any pipeline runtime failure
pub fn run(symbol: &str) -> anyhow::Result<()> {
    let cfg = load_analysis_config()?;

    // Validate symbol (re-homed from Config::validate() in Unit 3).
    let symbol = match crate::data::symbol::validate_symbol(symbol) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            anyhow::bail!("{e}");
        }
    };

    let target_date = Local::now().format("%Y-%m-%d").to_string();

    tracing::info!(
        quick_provider = %cfg.llm.quick_thinking_provider,
        deep_provider = %cfg.llm.deep_thinking_provider,
        symbol = %symbol,
        target_date = %target_date,
        "scorpio-analyst initialized"
    );

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to initialize async runtime: {e:#}");
            anyhow::bail!("failed to initialize async runtime: {e}");
        }
    };

    if let Err(e) = runtime.block_on(preflight_copilot_if_configured(
        &cfg.llm,
        &cfg.providers,
        &ProviderRateLimiters::from_config(&cfg.providers),
    )) {
        eprintln!("failed to preflight configured Copilot provider: {e:#}");
        return Err(e.into());
    }

    let snapshot_store = match runtime.block_on(SnapshotStore::from_config(&cfg)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to initialize snapshot storage: {e:#}");
            return Err(e.into());
        }
    };

    let rate_limiters = ProviderRateLimiters::from_config(&cfg.providers);

    let quick_handle = match create_completion_model(
        ModelTier::QuickThinking,
        &cfg.llm,
        &cfg.providers,
        &rate_limiters,
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to create quick-thinking model handle: {e:#}");
            anyhow::bail!("failed to create quick-thinking model handle: {e}");
        }
    };

    let deep_handle = match create_completion_model(
        ModelTier::DeepThinking,
        &cfg.llm,
        &cfg.providers,
        &rate_limiters,
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to create deep-thinking model handle: {e:#}");
            anyhow::bail!("failed to create deep-thinking model handle: {e}");
        }
    };

    let finnhub_limiter = SharedRateLimiter::finnhub_from_config(&cfg.rate_limits)
        .unwrap_or_else(|| SharedRateLimiter::disabled("finnhub"));
    let finnhub = match FinnhubClient::new(&cfg.api, finnhub_limiter) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to initialize Finnhub client: {e:#}");
            return Err(e.into());
        }
    };
    let fred_limiter = SharedRateLimiter::fred_from_config(&cfg.rate_limits)
        .unwrap_or_else(|| SharedRateLimiter::disabled("fred"));
    let fred = match FredClient::new(&cfg.api, fred_limiter) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to initialize FRED client: {e:#}");
            return Err(e.into());
        }
    };
    let yfinance = YFinanceClient::from_config(&cfg.rate_limits);

    let pipeline = TradingPipeline::new(
        cfg,
        finnhub,
        fred,
        yfinance,
        snapshot_store,
        quick_handle,
        deep_handle,
    );

    let initial_state = TradingState::new(symbol, &target_date);

    match runtime.block_on(pipeline.run_analysis_cycle(initial_state)) {
        Ok(state) => {
            if state.final_execution_status.is_none() {
                eprintln!("pipeline completed without a final execution status");
                anyhow::bail!("pipeline completed without a final execution status");
            }
            println!("{}", crate::report::format_final_report(&state));
        }
        Err(e) => {
            eprintln!("analysis cycle failed: {e:#}");
            return Err(e.into());
        }
    }

    Ok(())
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
        use crate::cli::setup::config_file::{PartialConfig, save_user_config_at};
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
