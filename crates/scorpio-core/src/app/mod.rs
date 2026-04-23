//! Application facade for one-shot analysis runs.
//!
//! [`AnalysisRuntime`] wraps the runtime-assembly sequence that every consumer
//! of `scorpio-core` would otherwise duplicate: provider preflight, snapshot
//! store initialization, per-tier completion-model handles, data-client
//! construction, rate-limiter wiring, and [`TradingPipeline`] assembly. Once
//! built, a single runtime can execute many analysis cycles via [`run`] without
//! re-assembling the pipeline.
//!
//! Terminal rendering (banner, figlet, final-report formatting) stays in the
//! CLI crate — this facade returns typed [`TradingState`] rather than a rendered
//! string so non-CLI consumers (TUI, backtest) can format output however they
//! like.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use scorpio_core::app::AnalysisRuntime;
//! use scorpio_core::config::Config;
//!
//! let cfg = Config::load()?;
//! let runtime = AnalysisRuntime::new(cfg).await?;
//! let state = runtime.run("AAPL").await?;
//! assert!(state.final_execution_status.is_some());
//! # Ok(())
//! # }
//! ```
//!
//! [`run`]: AnalysisRuntime::run

use anyhow::Context;

use crate::config::Config;
use crate::data::{FinnhubClient, FredClient, YFinanceClient, symbol::validate_symbol};
use crate::providers::ModelTier;
use crate::providers::factory::{create_completion_model, preflight_copilot_if_configured};
use crate::rate_limit::{ProviderRateLimiters, SharedRateLimiter};
use crate::state::TradingState;
use crate::workflow::{SnapshotStore, TradingPipeline};

/// One-shot analysis facade owning an assembled [`TradingPipeline`].
///
/// Construct with [`AnalysisRuntime::new`]; execute each analysis cycle via
/// [`AnalysisRuntime::run`]. The runtime is reusable — call `run` multiple
/// times with different symbols without rebuilding provider handles.
#[derive(Debug)]
pub struct AnalysisRuntime {
    /// Quick-thinking provider name, cached for the per-run structured log
    /// so consumers don't have to keep the full `Config` alive alongside the
    /// facade.
    quick_provider: String,
    /// Deep-thinking provider name, cached for the per-run structured log.
    deep_provider: String,
    /// Fully-assembled graph-flow pipeline. Owned by value; every `run`
    /// call executes a detached analysis cycle against it.
    pipeline: TradingPipeline,
}

impl AnalysisRuntime {
    /// Assemble providers, clients, snapshot store, and [`TradingPipeline`]
    /// from `cfg`.
    ///
    /// Must be called from inside a tokio runtime context. Returns any
    /// component-level assembly failure with the original context string
    /// preserved from the pre-facade CLI code.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any of the following fail:
    /// - Copilot provider preflight (when configured).
    /// - SQLite snapshot store initialization.
    /// - Quick- or deep-thinking completion-model handle creation.
    /// - Finnhub or FRED client construction.
    pub async fn new(cfg: Config) -> anyhow::Result<Self> {
        let quick_provider = cfg.llm.quick_thinking_provider.clone();
        let deep_provider = cfg.llm.deep_thinking_provider.clone();
        let rate_limiters = ProviderRateLimiters::from_config(&cfg.providers);

        preflight_copilot_if_configured(&cfg.llm, &cfg.providers, &rate_limiters)
            .await
            .context("failed to preflight configured Copilot provider")?;

        let snapshot_store = SnapshotStore::from_config(&cfg)
            .await
            .context("failed to initialize snapshot storage")?;

        let quick_handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg.llm,
            &cfg.providers,
            &rate_limiters,
        )
        .context("failed to create quick-thinking model handle")?;

        let deep_handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg.llm,
            &cfg.providers,
            &rate_limiters,
        )
        .context("failed to create deep-thinking model handle")?;

        let finnhub_limiter = SharedRateLimiter::finnhub_from_config(&cfg.rate_limits)
            .unwrap_or_else(|| SharedRateLimiter::disabled("finnhub"));
        let finnhub = FinnhubClient::new(&cfg.api, finnhub_limiter)
            .context("failed to initialize Finnhub client")?;

        let fred_limiter = SharedRateLimiter::fred_from_config(&cfg.rate_limits)
            .unwrap_or_else(|| SharedRateLimiter::disabled("fred"));
        let fred =
            FredClient::new(&cfg.api, fred_limiter).context("failed to initialize FRED client")?;

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

        Ok(Self {
            quick_provider,
            deep_provider,
            pipeline,
        })
    }

    /// Validate `symbol` and execute a single 5-phase analysis cycle.
    ///
    /// The symbol is re-validated here even if the caller already validated
    /// it, so non-CLI consumers still get the same input contract.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - `symbol` fails [`validate_symbol`] (invalid format).
    /// - The pipeline's `run_analysis_cycle` returns an error.
    /// - The pipeline completes without producing a final execution status.
    pub async fn run(&self, symbol: &str) -> anyhow::Result<TradingState> {
        let symbol = validate_symbol(symbol)?;
        let target_date = chrono::Local::now().format("%Y-%m-%d").to_string();

        tracing::info!(
            quick_provider = %self.quick_provider,
            deep_provider = %self.deep_provider,
            symbol = %symbol,
            target_date = %target_date,
            "scorpio-analyst initialized"
        );

        let initial_state = TradingState::new(symbol, &target_date);
        let state = self
            .pipeline
            .run_analysis_cycle(initial_state)
            .await
            .context("analysis cycle failed")?;

        if state.final_execution_status.is_none() {
            anyhow::bail!("pipeline completed without a final execution status");
        }

        Ok(state)
    }

    /// Hermetic test seam: wrap a prebuilt [`TradingPipeline`] without paying
    /// the assembly cost of [`AnalysisRuntime::new`].
    ///
    /// Integration tests and downstream test-harnesses use this to exercise
    /// [`AnalysisRuntime::run`] against the existing `workflow::test_support`
    /// stubbed tasks; production callers only use [`AnalysisRuntime::new`].
    ///
    /// The cached provider names are empty strings here because stubbed tests
    /// do not need the init log.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn from_pipeline(pipeline: TradingPipeline) -> Self {
        Self {
            quick_provider: String::new(),
            deep_provider: String::new(),
            pipeline,
        }
    }
}
