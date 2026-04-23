pub mod analyze;
pub mod setup;
pub mod update;

use std::path::PathBuf;

use clap::builder::FalseyValueParser;
use clap::{Args, Parser, Subcommand};

/// Scorpio Analyst — multi-agent LLM-powered financial analysis.
#[derive(Debug, Parser)]
#[command(name = "scorpio", bin_name = "scorpio", version, about)]
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
    Analyze(AnalyzeArgs),
    /// Interactive wizard to configure API keys and provider routing.
    Setup,
    /// Upgrade scorpio to the latest release from GitHub.
    Upgrade,
}

/// Arguments for `scorpio analyze`.
#[derive(Debug, Clone, Default, Args)]
pub struct AnalyzeArgs {
    /// Ticker symbol to analyze (e.g. AAPL, NVDA, BTC-USD).
    #[arg(value_name = "SYMBOL")]
    pub symbol: String,

    /// Suppress the analyze banner and terminal reporter.
    /// Requires another reporter such as --json to be enabled.
    #[arg(long = "no-terminal")]
    pub no_terminal: bool,

    /// Write a pretty-printed JSON artifact to --output-dir.
    #[arg(long)]
    pub json: bool,

    /// Directory for file-based reporters.
    /// Defaults to ~/.scorpio-analyst/reports and is created if missing.
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    #[test]
    fn parse_analyze_with_symbol() {
        let cli = Cli::try_parse_from(["scorpio", "analyze", "AAPL"]).unwrap();
        assert!(matches!(&cli.command, Commands::Analyze(args) if args.symbol == "AAPL"));
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
        assert!(matches!(&cli.command, Commands::Analyze(args) if args.symbol == "AAPL"));
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

    #[test]
    fn command_reports_stable_cli_name_in_help() {
        let mut rendered = Vec::new();
        let mut cmd = Cli::command();
        cmd.write_long_help(&mut rendered)
            .expect("help should render");

        let help = String::from_utf8(rendered).expect("help should be utf8");
        assert!(
            help.contains("Usage: scorpio [OPTIONS] <COMMAND>"),
            "help output should use the installed `scorpio` command name; got: {help}"
        );
        assert!(
            !help.contains("Usage: scorpio-cli "),
            "help output should not leak the package name; got: {help}"
        );
    }

    #[test]
    fn analyze_subcommand_reports_stable_cli_name_in_help() {
        let err = Cli::try_parse_from(["scorpio", "analyze", "--help"])
            .expect_err("--help should exit through clap");
        assert_eq!(err.kind(), ErrorKind::DisplayHelp);

        let help = err.to_string();
        assert!(
            help.contains("Usage: scorpio analyze [OPTIONS] <SYMBOL>"),
            "subcommand help should use the installed `scorpio` command name; got: {help}"
        );
        assert!(
            !help.contains("Usage: scorpio-cli analyze"),
            "subcommand help should not leak the package name; got: {help}"
        );
    }

    #[test]
    fn command_reports_stable_cli_name_in_version() {
        let version = Cli::command().render_version().to_string();
        assert!(
            version.starts_with("scorpio "),
            "version output should use the installed `scorpio` command name; got: {version}"
        );
        assert!(
            !version.starts_with("scorpio-cli "),
            "version output should not leak the package name; got: {version}"
        );
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
