//! Trading pipeline orchestration using graph-flow.
//!
//! [`TradingPipeline`] wires all five agent phases into a directed graph and
//! [`runtime::run_analysis_cycle`] is the single entry point for callers.
//!
//! # Pipeline topology
//!
//! ```text
//! FanOutTask[analysts] ──► AnalystSyncTask
//!                                │
//!             ┌──max_debate_rounds > 0──┐
//!             ▼                         ▼
//!  BullishResearcherTask    DebateModeratorTask (direct, skip debate)
//!             │
//!  BearishResearcherTask
//!             │
//!  DebateModeratorTask ──► (loop back if debate_round < max)
//!             │ (else)
//!             ▼
//!       TraderTask
//!             │
//!   ┌─max_risk_rounds > 0──┐
//!   ▼                       ▼
//! AggressiveRiskTask  RiskModeratorTask (direct, skip risk)
//!   │
//! ConservativeRiskTask
//!   │
//! NeutralRiskTask
//!   │
//! RiskModeratorTask ──► (loop back if risk_round < max)
//!   │ (else)
//!   ▼
//! FundManagerTask
//! ```

pub(crate) mod constants;
mod errors;
pub(crate) mod runtime;

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-helpers"))]
pub use errors::map_graph_error;

use std::sync::Arc;

use graph_flow::Graph;
#[cfg(any(test, feature = "test-helpers"))]
use thiserror::Error;

use crate::{
    analysis_packs::RuntimePolicy,
    config::Config,
    data::{
        FinnhubClient, FredClient, SecEdgarClient, YFinanceClient,
        adapters::catalysts::CatalystCalendarProvider,
    },
    providers::factory::CompletionModelHandle,
    rate_limit::SharedRateLimiter,
    workflow::SnapshotStore,
};

#[cfg(any(test, feature = "test-helpers"))]
use constants::REPLACEABLE_TASK_IDS;

/// Hard ceiling on `FlowRunner::run()` iterations inside
/// [`runtime::run_analysis_cycle`].
///
/// The pipeline has ~12 distinct tasks. With `max_debate_rounds` and
/// `max_risk_rounds` each allowing up to ~10 rounds, the theoretical maximum
/// for a legitimate run is around 50 steps. `200` provides comfortable headroom
/// while still catching runaway loops caused by corrupted round counters or
/// misconfigured conditional edges.
pub(crate) const MAX_PIPELINE_STEPS: usize = 200;

const _: () = {
    assert!(
        MAX_PIPELINE_STEPS >= 100,
        "ceiling too low - may cause false positives"
    );
    assert!(
        MAX_PIPELINE_STEPS <= 1000,
        "ceiling too high - may not catch runaways quickly enough"
    );
};

#[cfg(any(test, feature = "test-helpers"))]
#[derive(Debug, Error, Clone, PartialEq, Eq)]
/// Errors raised by test-only task replacement helpers.
pub enum WorkflowTestSeamError {
    #[error("unknown workflow task id '{task_id}' cannot be replaced via test seam")]
    UnknownTaskId { task_id: String },
}

/// Construct the default `Arc<SecEdgarClient>` used by [`AnalystSyncTask`].
///
/// Mirrors the rate-limit policy already used by `build_catalyst_provider`
/// (10 rps under the `"sec-edgar"` label). `SecEdgarClient::new` only fails
/// when `reqwest::Client::builder()` fails — virtually impossible in practice,
/// so we `.expect(...)` rather than thread an additional `Result` through
/// every pipeline-construction call site. The downstream consumer is
/// `Option<Arc<SecEdgarClient>>` on `AnalystSyncTask` so the field can still
/// be elided in narrow test paths via `AnalystSyncTask::with_yfinance`.
fn build_default_sec_edgar_client() -> Arc<SecEdgarClient> {
    Arc::new(
        SecEdgarClient::new(SharedRateLimiter::new("sec-edgar", 10))
            .expect("SecEdgarClient construction must succeed (reqwest builder)"),
    )
}

/// Orchestrates the full five-phase trading analysis pipeline.
///
/// The graph is built once in [`new`][Self::new] and reused across analysis
/// cycles. Use [`runtime::run_analysis_cycle`] as the single entry point for
/// callers.
pub struct TradingPipeline {
    pub(super) config: Arc<Config>,
    pub(super) finnhub: FinnhubClient,
    pub(super) fred: FredClient,
    pub(super) yfinance: YFinanceClient,
    /// Alpha Vantage transcript provider. `None` when no key is configured
    /// or transcripts are disabled; downstream renders "Unavailable" prompt
    /// language in that case.
    pub(super) alpha_vantage: Option<crate::data::AlphaVantageClient>,
    pub(super) catalyst_provider: Arc<dyn CatalystCalendarProvider>,
    pub(super) snapshot_store: Arc<SnapshotStore>,
    /// Handle for quick-thinking agents (Analyst Team - Phase 1).
    pub(super) quick_handle: CompletionModelHandle,
    /// Handle for deep-thinking agents (Researcher, Trader, Risk Team, Fund Manager).
    pub(super) deep_handle: CompletionModelHandle,
    /// Pack-derived runtime policy when the pipeline was built from a resolved pack.
    pub(super) runtime_policy: Option<RuntimePolicy>,
    /// Pre-built graph - stateless, safe to share across analysis cycles.
    pub(super) graph: Arc<Graph>,
}

impl std::fmt::Debug for TradingPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TradingPipeline")
            .field("config", &self.config)
            .field("finnhub", &self.finnhub)
            .field("fred", &self.fred)
            .field("yfinance", &self.yfinance)
            .field(
                "alpha_vantage",
                &self.alpha_vantage.as_ref().map(|c| format!("{c:?}")),
            )
            .field("catalyst_provider", &"Arc<dyn CatalystCalendarProvider>")
            .field("snapshot_store", &self.snapshot_store)
            .field("quick_handle", &self.quick_handle)
            .field("deep_handle", &self.deep_handle)
            .field("runtime_policy", &self.runtime_policy)
            .field("graph", &"Arc<Graph>")
            .finish()
    }
}

impl TradingPipeline {
    /// Construct a new pipeline.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let pipeline = TradingPipeline::new(
    ///     config,
    ///     finnhub,
    ///     fred,
    ///     yfinance,
    ///     snapshot_store,
    ///     quick_handle,
    ///     deep_handle,
    /// );
    /// ```
    pub fn new(
        config: Config,
        finnhub: FinnhubClient,
        fred: FredClient,
        yfinance: YFinanceClient,
        snapshot_store: SnapshotStore,
        quick_handle: CompletionModelHandle,
        deep_handle: CompletionModelHandle,
    ) -> Self {
        let config = Arc::new(config);
        let snapshot_store = Arc::new(snapshot_store);
        let runtime_policy =
            crate::analysis_packs::resolve_runtime_policy(&config.analysis_pack).ok();
        let catalyst_provider = runtime::build_catalyst_provider(
            &finnhub,
            &fred,
            &yfinance,
            std::time::Duration::from_secs(config.enrichment.fetch_timeout_secs),
        );
        let sec_edgar = build_default_sec_edgar_client();
        let graph = runtime::build_graph(
            Arc::clone(&config),
            &finnhub,
            &fred,
            &yfinance,
            sec_edgar,
            Arc::clone(&snapshot_store),
            &quick_handle,
            &deep_handle,
        );
        Self {
            config,
            finnhub,
            fred,
            yfinance,
            alpha_vantage: None,
            catalyst_provider,
            snapshot_store,
            quick_handle,
            deep_handle,
            runtime_policy,
            graph,
        }
    }

    /// Construct a pipeline, surfacing pack-resolution failures as a typed
    /// error instead of silently coercing them away.
    ///
    /// Production callers (`AnalysisRuntime::run`, the `scorpio analyze` CLI,
    /// backtest entries) route through `try_new` so an invalid
    /// `config.analysis_pack` value fails before any graph node executes.
    /// `PreflightTask` would otherwise reject the run with a generic "pack
    /// resolution failed" error; `try_new` produces a clearer diagnostic at
    /// pipeline-construction time.
    ///
    /// `new` remains available for tests and documents the legacy
    /// behavior (silently coerces an invalid pack id to "no runtime policy"
    /// and lets `PreflightTask` issue the eventual diagnostic).
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::Config`] when `config.analysis_pack` does not
    /// resolve to a registered pack.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        config: Config,
        finnhub: FinnhubClient,
        fred: FredClient,
        yfinance: YFinanceClient,
        alpha_vantage: Option<crate::data::AlphaVantageClient>,
        snapshot_store: SnapshotStore,
        quick_handle: CompletionModelHandle,
        deep_handle: CompletionModelHandle,
    ) -> Result<Self, crate::error::TradingError> {
        let config = Arc::new(config);
        let snapshot_store = Arc::new(snapshot_store);
        let runtime_policy = crate::analysis_packs::resolve_runtime_policy(&config.analysis_pack)
            .map_err(|e| {
            crate::error::TradingError::Config(anyhow::anyhow!(
                "TradingPipeline::try_new: invalid analysis_pack {:?}: {e}",
                config.analysis_pack
            ))
        })?;
        let catalyst_provider = runtime::build_catalyst_provider(
            &finnhub,
            &fred,
            &yfinance,
            std::time::Duration::from_secs(config.enrichment.fetch_timeout_secs),
        );
        let sec_edgar = build_default_sec_edgar_client();
        let graph = runtime::build_graph(
            Arc::clone(&config),
            &finnhub,
            &fred,
            &yfinance,
            sec_edgar,
            Arc::clone(&snapshot_store),
            &quick_handle,
            &deep_handle,
        );
        Ok(Self {
            config,
            finnhub,
            fred,
            yfinance,
            alpha_vantage,
            catalyst_provider,
            snapshot_store,
            quick_handle,
            deep_handle,
            runtime_policy: Some(runtime_policy),
            graph,
        })
    }

    /// Assemble a pipeline from pre-built parts, including the graph.
    ///
    /// Used by `workflow::builder::TradingPipeline::from_pack` so the Phase 7
    /// pack-driven entry point can share the same field layout without
    /// re-exposing the private fields.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn __from_parts(
        config: Arc<Config>,
        finnhub: FinnhubClient,
        fred: FredClient,
        yfinance: YFinanceClient,
        alpha_vantage: Option<crate::data::AlphaVantageClient>,
        catalyst_provider: Arc<dyn CatalystCalendarProvider>,
        snapshot_store: Arc<SnapshotStore>,
        quick_handle: CompletionModelHandle,
        deep_handle: CompletionModelHandle,
        graph: Arc<Graph>,
        runtime_policy: Option<RuntimePolicy>,
    ) -> Self {
        Self {
            config,
            finnhub,
            fred,
            yfinance,
            alpha_vantage,
            catalyst_provider,
            snapshot_store,
            quick_handle,
            deep_handle,
            runtime_policy,
            graph,
        }
    }

    #[cfg(any(test, feature = "test-helpers"))]
    /// Build and return a fresh [`Graph`] with the pipeline topology for tests.
    pub fn build_graph(&self) -> Arc<Graph> {
        runtime::build_graph(
            Arc::clone(&self.config),
            &self.finnhub,
            &self.fred,
            &self.yfinance,
            build_default_sec_edgar_client(),
            Arc::clone(&self.snapshot_store),
            &self.quick_handle,
            &self.deep_handle,
        )
    }

    #[cfg(any(test, feature = "test-helpers"))]
    pub fn replace_catalyst_provider_for_test(
        &mut self,
        provider: Arc<dyn CatalystCalendarProvider>,
    ) {
        self.catalyst_provider = provider;
    }

    #[cfg(any(test, feature = "test-helpers"))]
    /// Replace a pipeline task with a test double.
    pub fn replace_task_for_test(
        &self,
        task: Arc<dyn graph_flow::Task>,
    ) -> Result<(), WorkflowTestSeamError> {
        let task_id = task.id();
        if !REPLACEABLE_TASK_IDS.contains(&task_id) {
            return Err(WorkflowTestSeamError::UnknownTaskId {
                task_id: task_id.to_owned(),
            });
        }
        self.graph.add_task(task);
        Ok(())
    }

    #[cfg(any(test, feature = "test-helpers"))]
    /// Install the standard workflow stub task set for integration tests.
    pub fn install_stub_tasks_for_test(&self) -> Result<(), WorkflowTestSeamError> {
        crate::workflow::tasks::test_helpers::replace_with_stubs(
            self,
            Arc::clone(&self.snapshot_store),
        )
    }

    #[cfg(any(test, feature = "test-helpers"))]
    /// Install deterministic workflow stubs but keep the real auditor task.
    pub fn install_stub_tasks_except_auditor_for_test(&self) -> Result<(), WorkflowTestSeamError> {
        crate::workflow::tasks::test_helpers::replace_with_stubs_except_auditor(
            self,
            Arc::clone(&self.snapshot_store),
        )
    }
}
