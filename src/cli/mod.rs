pub mod analyze;
pub mod setup;

use clap::{Parser, Subcommand};

/// Scorpio Analyst — multi-agent LLM-powered financial analysis.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the full 5-phase analysis pipeline for a ticker symbol.
    Analyze {
        /// Ticker symbol to analyse (e.g. AAPL, NVDA, BTC-USD).
        #[arg(value_name = "SYMBOL")]
        symbol: String,
    },
    /// Interactive wizard to configure API keys and provider routing.
    Setup,
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
}
