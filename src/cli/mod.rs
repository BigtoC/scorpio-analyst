pub mod analyze;
pub mod setup;
pub mod update;

use clap::builder::FalseyValueParser;
use clap::{Parser, Subcommand};

/// Scorpio Analyst — multi-agent LLM-powered financial analysis.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Suppress the background release check.
    ///
    /// Also controlled by `SCORPIO_NO_UPDATE_CHECK=1|true|yes|on` (and false-ish
    /// equivalents). Uses clap's `FalseyValueParser` so any non-false-ish env
    /// value enables suppression instead of producing a hard CLI error.
    #[arg(
        long,
        global = true,
        env = "SCORPIO_NO_UPDATE_CHECK",
        value_parser = FalseyValueParser::new(),
        default_value_t = false
    )]
    pub no_update_check: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the full 5-phase analysis pipeline for a ticker symbol.
    Analyze {
        /// Ticker symbol to analyze (e.g. AAPL, NVDA, BTC-USD).
        #[arg(value_name = "SYMBOL")]
        symbol: String,
    },
    /// Interactive wizard to configure API keys and provider routing.
    Setup,
    /// Upgrade scorpio to the latest release from GitHub.
    Upgrade,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn parse_analyze_with_symbol() {
        let cli = Cli::try_parse_from(["scorpio", "analyze", "AAPL"]).unwrap();
        assert!(matches!(cli.command, Commands::Analyze { symbol } if symbol == "AAPL"));
    }

    #[test]
    fn parse_setup_subcommand() {
        let cli = Cli::try_parse_from(["scorpio", "setup"]).unwrap();
        assert!(matches!(cli.command, Commands::Setup));
    }

    #[test]
    fn parse_upgrade_subcommand() {
        let cli = Cli::try_parse_from(["scorpio", "upgrade"]).unwrap();
        assert!(matches!(cli.command, Commands::Upgrade));
    }

    #[test]
    fn parse_no_update_check_before_subcommand() {
        let cli = Cli::try_parse_from(["scorpio", "--no-update-check", "analyze", "AAPL"]).unwrap();
        assert!(cli.no_update_check);
        assert!(matches!(cli.command, Commands::Analyze { symbol } if symbol == "AAPL"));
    }

    #[test]
    fn parse_no_update_check_after_subcommand_is_global() {
        let cli = Cli::try_parse_from(["scorpio", "analyze", "AAPL", "--no-update-check"]).unwrap();
        assert!(cli.no_update_check);
    }

    #[test]
    fn parse_help_subcommand_yields_display_help_error() {
        let err = Cli::try_parse_from(["scorpio", "help"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    }

    #[test]
    fn parse_no_subcommand_yields_error() {
        // Clap emits DisplayHelpOnMissingArgumentOrSubcommand (exits 0 at runtime
        // by printing help) when no subcommand is given with version/about enabled.
        let err = Cli::try_parse_from(["scorpio"]).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ),
            "expected missing-subcommand-style error, got: {:?}",
            err.kind()
        );
    }

    #[test]
    fn parse_analyze_without_symbol_yields_missing_required_argument_error() {
        let err = Cli::try_parse_from(["scorpio", "analyze"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    // ── env-var suppression ─────────────────────────────────────────────

    /// All SCORPIO_NO_UPDATE_CHECK tests run in one `#[test]` so they never race
    /// on the process-wide env var. Parallel tests setting/unsetting the same env
    /// var produced flaky assertions; serialising within a single test function
    /// is the simplest fix and costs nothing.
    #[test]
    fn env_var_boolish_parsing_matrix() {
        fn parse_with_env(val: &str) -> Result<Cli, clap::Error> {
            // Safety: test-only; tests are sequential within this function.
            unsafe {
                std::env::set_var("SCORPIO_NO_UPDATE_CHECK", val);
            }
            let res = Cli::try_parse_from(["scorpio", "analyze", "AAPL"]);
            unsafe {
                std::env::remove_var("SCORPIO_NO_UPDATE_CHECK");
            }
            res
        }

        // Truthy values accepted by FalseyValueParser (npm-style matrix).
        for val in ["1", "true", "yes", "on", "y", "enabled"] {
            let cli = parse_with_env(val).unwrap_or_else(|e| panic!("val={val}: {e}"));
            assert!(cli.no_update_check, "val={val} should enable suppression");
        }

        // Falsy values.
        for val in ["0", "false", "no", "off", "n"] {
            let cli = parse_with_env(val).unwrap_or_else(|e| panic!("val={val}: {e}"));
            assert!(
                !cli.no_update_check,
                "val={val} should leave suppression off"
            );
        }

        // Arbitrary non-false-ish values also enable suppression instead of
        // turning a malformed env var into a CLI parse failure.
        let cli = parse_with_env("random").unwrap_or_else(|e| panic!("val=random: {e}"));
        assert!(cli.no_update_check);
    }
}
