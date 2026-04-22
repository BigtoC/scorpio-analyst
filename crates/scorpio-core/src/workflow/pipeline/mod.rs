//! Trading pipeline orchestration using graph-flow.
//!
//! [`TradingPipeline`] wires all five agent phases into a directed graph and
//! provides [`TradingPipeline::run_analysis_cycle`] as the single entry point
//! for callers.
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

mod constants;
mod errors;
mod runtime;

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-helpers"))]
pub use errors::map_graph_error;

use std::sync::Arc;

use graph_flow::Graph;
#[cfg(any(test, feature = "test-helpers"))]
use thiserror::Error;

use crate::{
    config::Config,
    data::{FinnhubClient, FredClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    workflow::SnapshotStore,
};

#[cfg(any(test, feature = "test-helpers"))]
use constants::REPLACEABLE_TASK_IDS;

/// Hard ceiling on `FlowRunner::run()` iterations inside
/// [`TradingPipeline::run_analysis_cycle`].
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

/// Orchestrates the full five-phase trading analysis pipeline.
///
/// The graph is built once in [`new`][Self::new] and reused across analysis
/// cycles. Use [`run_analysis_cycle`][Self::run_analysis_cycle] as the single
/// entry point for callers.
pub struct TradingPipeline {
    pub(super) config: Arc<Config>,
    pub(super) finnhub: FinnhubClient,
    pub(super) fred: FredClient,
    pub(super) yfinance: YFinanceClient,
    pub(super) snapshot_store: Arc<SnapshotStore>,
    /// Handle for quick-thinking agents (Analyst Team - Phase 1).
    pub(super) quick_handle: CompletionModelHandle,
    /// Handle for deep-thinking agents (Researcher, Trader, Risk Team, Fund Manager).
    pub(super) deep_handle: CompletionModelHandle,
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
            .field("snapshot_store", &self.snapshot_store)
            .field("quick_handle", &self.quick_handle)
            .field("deep_handle", &self.deep_handle)
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
        let graph = runtime::build_graph(
            Arc::clone(&config),
            &finnhub,
            &fred,
            &yfinance,
            Arc::clone(&snapshot_store),
            &quick_handle,
            &deep_handle,
        );
        Self {
            config,
            finnhub,
            fred,
            yfinance,
            snapshot_store,
            quick_handle,
            deep_handle,
            graph,
        }
    }

    /// Run a full analysis cycle for the given initial state.
    ///
    /// 1. Resets per-cycle outputs on the provided state.
    /// 2. Canonicalizes the runtime symbol before any best-effort prefetch.
    /// 3. Seeds a fresh in-memory session with the serialized `TradingState`.
    /// 4. Runs the `FlowRunner` loop until the pipeline completes.
    /// 5. Deserializes and returns the final `TradingState`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::TradingError::SchemaViolation`] when the input
    /// symbol is invalid, and [`crate::error::TradingError::GraphFlow`] on any
    /// graph/session/task orchestration failure.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let final_state = pipeline.run_analysis_cycle(TradingState::new("AAPL", "2026-03-20")).await?;
    /// ```
    pub async fn run_analysis_cycle(
        &self,
        initial_state: crate::state::TradingState,
    ) -> Result<crate::state::TradingState, crate::error::TradingError> {
        runtime::run_analysis_cycle(self, initial_state).await
    }

    #[cfg(any(test, feature = "test-helpers"))]
    /// Build and return a fresh [`Graph`] with the pipeline topology for tests.
    pub fn build_graph(&self) -> Arc<Graph> {
        runtime::build_graph(
            Arc::clone(&self.config),
            &self.finnhub,
            &self.fred,
            &self.yfinance,
            Arc::clone(&self.snapshot_store),
            &self.quick_handle,
            &self.deep_handle,
        )
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
}
