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

use std::sync::Arc;

use graph_flow::{
    ExecutionStatus, FlowRunner, Graph, InMemorySessionStorage, Session, SessionStorage,
    fanout::FanOutTask,
};
#[cfg(any(test, feature = "test-helpers"))]
use thiserror::Error;
use tracing::{error, info, instrument};
use uuid::Uuid;

use crate::{
    config::Config,
    data::{FinnhubClient, FredClient, YFinanceClient},
    error::TradingError,
    providers::factory::{CompletionModelHandle, sanitize_error_summary},
    state::TradingState,
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        tasks::{
            AggressiveRiskTask, AnalystSyncTask, BearishResearcherTask, BullishResearcherTask,
            ConservativeRiskTask, DebateModeratorTask, FundManagerTask, FundamentalAnalystTask,
            KEY_CACHED_NEWS, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
            KEY_RISK_ROUND, NeutralRiskTask, NewsAnalystTask, RiskModeratorTask,
            SentimentAnalystTask, TechnicalAnalystTask,
        },
    },
};

// ── Graph task-ID constants ──────────────────────────────────────────────────

const TASK_ANALYST_FAN_OUT: &str = "analyst_fanout";
const TASK_ANALYST_SYNC: &str = "analyst_sync";
const TASK_BULLISH_RESEARCHER: &str = "bullish_researcher";
const TASK_BEARISH_RESEARCHER: &str = "bearish_researcher";
const TASK_DEBATE_MODERATOR: &str = "debate_moderator";
const TASK_TRADER: &str = "trader";
const TASK_AGGRESSIVE_RISK: &str = "aggressive_risk";
const TASK_CONSERVATIVE_RISK: &str = "conservative_risk";
const TASK_NEUTRAL_RISK: &str = "neutral_risk";
const TASK_RISK_MODERATOR: &str = "risk_moderator";
const TASK_FUND_MANAGER: &str = "fund_manager";

#[cfg(any(test, feature = "test-helpers"))]
const REPLACEABLE_TASK_IDS: [&str; 11] = [
    TASK_ANALYST_FAN_OUT,
    TASK_ANALYST_SYNC,
    TASK_BULLISH_RESEARCHER,
    TASK_BEARISH_RESEARCHER,
    TASK_DEBATE_MODERATOR,
    TASK_TRADER,
    TASK_AGGRESSIVE_RISK,
    TASK_CONSERVATIVE_RISK,
    TASK_NEUTRAL_RISK,
    TASK_RISK_MODERATOR,
    TASK_FUND_MANAGER,
];

/// Hard ceiling on `FlowRunner::run()` iterations inside
/// [`TradingPipeline::run_analysis_cycle`].
///
/// The pipeline has ~11 distinct tasks.  With `max_debate_rounds` and
/// `max_risk_rounds` each allowing up to ~10 rounds, the theoretical maximum
/// for a legitimate run is around 50 steps.  200 provides comfortable headroom
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
pub enum WorkflowTestSeamError {
    #[error("unknown workflow task id '{task_id}' cannot be replaced via test seam")]
    UnknownTaskId { task_id: String },
}

// ── TradingPipeline ──────────────────────────────────────────────────────────

/// Orchestrates the full five-phase trading analysis pipeline.
///
/// The graph is built once in [`new`][Self::new] and reused across analysis
/// cycles. Use [`run_analysis_cycle`][Self::run_analysis_cycle] as the single
/// entry point for callers.
pub struct TradingPipeline {
    config: Arc<Config>,
    finnhub: FinnhubClient,
    fred: FredClient,
    yfinance: YFinanceClient,
    snapshot_store: Arc<SnapshotStore>,
    /// Handle for quick-thinking agents (Analyst Team — Phase 1).
    quick_handle: CompletionModelHandle,
    /// Handle for deep-thinking agents (Researcher, Trader, Risk Team, Fund Manager).
    deep_handle: CompletionModelHandle,
    /// Pre-built graph — stateless, safe to share across analysis cycles.
    graph: Arc<Graph>,
}

impl TradingPipeline {
    /// Construct a new pipeline.
    ///
    /// # Parameters
    ///
    /// - `config` — application configuration (will be `Arc`-wrapped internally)
    /// - `finnhub` — Finnhub API client (used by analyst tasks)
    /// - `fred` — FRED API client (used by the news analyst's macro tool)
    /// - `yfinance` — yfinance client (used by the technical analyst)
    /// - `snapshot_store` — SQLite-backed snapshot store for phase persistence
    /// - `quick_handle` — pre-built completion-model handle for quick-thinking agents (Phase 1)
    /// - `deep_handle` — pre-built completion-model handle for deep-thinking agents (Phases 2–5)
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
        let graph = Self::build_graph_impl(
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

    /// Build and return a fresh [`Graph`] with the pipeline topology.
    ///
    /// Primarily useful for tests that need to inspect the graph topology
    /// without gaining a mutable handle to the live graph used by
    /// [`run_analysis_cycle`][Self::run_analysis_cycle].
    pub fn build_graph(&self) -> Arc<Graph> {
        Self::build_graph_impl(
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
    pub fn replace_task_for_test(
        &self,
        task: Arc<dyn graph_flow::Task>,
    ) -> Result<(), WorkflowTestSeamError> {
        let task_id = task.id();
        if !is_replaceable_task_id(task_id) {
            return Err(WorkflowTestSeamError::UnknownTaskId {
                task_id: task_id.to_owned(),
            });
        }
        self.graph.add_task(task);
        Ok(())
    }

    #[cfg(any(test, feature = "test-helpers"))]
    pub fn install_stub_tasks_for_test(&self) -> Result<(), WorkflowTestSeamError> {
        crate::workflow::tasks::test_helpers::replace_with_stubs(
            self,
            Arc::clone(&self.snapshot_store),
        )
    }

    /// Build the directed [`Graph`] for the trading pipeline.
    ///
    /// This is a private associated function called once from [`new`][Self::new].
    /// Phase 1 analyst tasks use `quick_handle`; all other phases use `deep_handle`.
    fn build_graph_impl(
        config: Arc<Config>,
        finnhub: &FinnhubClient,
        fred: &FredClient,
        yfinance: &YFinanceClient,
        snapshot_store: Arc<SnapshotStore>,
        quick_handle: &CompletionModelHandle,
        deep_handle: &CompletionModelHandle,
    ) -> Arc<Graph> {
        let graph = Arc::new(Graph::new("trading_pipeline"));

        // ── Phase 1: analyst fan-out (QUICK handle) ───────────────────────
        let fan_out = FanOutTask::new(
            TASK_ANALYST_FAN_OUT,
            vec![
                FundamentalAnalystTask::new(
                    quick_handle.clone(),
                    finnhub.clone(),
                    config.llm.clone(),
                ),
                SentimentAnalystTask::new(
                    quick_handle.clone(),
                    finnhub.clone(),
                    config.llm.clone(),
                ),
                NewsAnalystTask::new(
                    quick_handle.clone(),
                    finnhub.clone(),
                    fred.clone(),
                    config.llm.clone(),
                ),
                TechnicalAnalystTask::new(
                    quick_handle.clone(),
                    yfinance.clone(),
                    config.llm.clone(),
                ),
            ],
        );
        graph.add_task(fan_out);

        // ── Phase 1 sync: aggregation + degradation ───────────────────────
        let analyst_sync = AnalystSyncTask::new(Arc::clone(&snapshot_store));
        graph.add_task(analyst_sync);
        graph.add_edge(TASK_ANALYST_FAN_OUT, TASK_ANALYST_SYNC);

        // Conditional: if max_debate_rounds > 0, start debate; else jump
        // straight to DebateModeratorTask so it can still produce a consensus
        // from analyst data alone.
        graph.add_conditional_edge(
            TASK_ANALYST_SYNC,
            |ctx| ctx.get_sync::<u32>(KEY_MAX_DEBATE_ROUNDS).unwrap_or(0) > 0,
            TASK_BULLISH_RESEARCHER, // yes: run debate
            TASK_DEBATE_MODERATOR,   // no:  skip to moderator
        );

        // ── Phase 2: researcher debate (DEEP handle) ──────────────────────
        let bullish = BullishResearcherTask::new(Arc::clone(&config), deep_handle.clone());
        let bearish = BearishResearcherTask::new(Arc::clone(&config), deep_handle.clone());
        let debate_mod = DebateModeratorTask::new(
            Arc::clone(&config),
            deep_handle.clone(),
            Arc::clone(&snapshot_store),
        );

        graph.add_task(bullish);
        graph.add_task(bearish);
        graph.add_task(debate_mod);

        graph.add_edge(TASK_BULLISH_RESEARCHER, TASK_BEARISH_RESEARCHER);
        graph.add_edge(TASK_BEARISH_RESEARCHER, TASK_DEBATE_MODERATOR);

        // Conditional: if debate_round < max_debate_rounds, loop back for
        // another round; otherwise advance to TraderTask.
        graph.add_conditional_edge(
            TASK_DEBATE_MODERATOR,
            |ctx| {
                let round = ctx.get_sync::<u32>(KEY_DEBATE_ROUND).unwrap_or(0);
                let max = ctx.get_sync::<u32>(KEY_MAX_DEBATE_ROUNDS).unwrap_or(0);
                round < max
            },
            TASK_BULLISH_RESEARCHER, // yes: more rounds
            TASK_TRADER,             // no:  move to trader
        );

        // ── Phase 3: trader (Config builds handle internally) ─────────────
        let trader = crate::workflow::tasks::TraderTask::new(
            Arc::clone(&config),
            Arc::clone(&snapshot_store),
        );
        graph.add_task(trader);

        // Conditional: if max_risk_rounds > 0, run risk discussion; else
        // jump straight to RiskModeratorTask for a synthesis pass.
        graph.add_conditional_edge(
            TASK_TRADER,
            |ctx| ctx.get_sync::<u32>(KEY_MAX_RISK_ROUNDS).unwrap_or(0) > 0,
            TASK_AGGRESSIVE_RISK, // yes: run risk
            TASK_RISK_MODERATOR,  // no:  skip to moderator
        );

        // ── Phase 4: risk discussion — sequential within each round (DEEP handle)
        let aggressive = AggressiveRiskTask::new(Arc::clone(&config), deep_handle.clone());
        let conservative = ConservativeRiskTask::new(Arc::clone(&config), deep_handle.clone());
        let neutral = NeutralRiskTask::new(Arc::clone(&config), deep_handle.clone());
        let risk_mod = RiskModeratorTask::new(
            Arc::clone(&config),
            deep_handle.clone(),
            Arc::clone(&snapshot_store),
        );

        graph.add_task(aggressive);
        graph.add_task(conservative);
        graph.add_task(neutral);
        graph.add_task(risk_mod);

        graph.add_edge(TASK_AGGRESSIVE_RISK, TASK_CONSERVATIVE_RISK);
        graph.add_edge(TASK_CONSERVATIVE_RISK, TASK_NEUTRAL_RISK);
        graph.add_edge(TASK_NEUTRAL_RISK, TASK_RISK_MODERATOR);

        // Conditional: if risk_round < max_risk_rounds, loop back for
        // another round; otherwise advance to FundManagerTask.
        graph.add_conditional_edge(
            TASK_RISK_MODERATOR,
            |ctx| {
                let round = ctx.get_sync::<u32>(KEY_RISK_ROUND).unwrap_or(0);
                let max = ctx.get_sync::<u32>(KEY_MAX_RISK_ROUNDS).unwrap_or(0);
                round < max
            },
            TASK_AGGRESSIVE_RISK, // yes: more rounds
            TASK_FUND_MANAGER,    // no:  move to fund manager
        );

        // ── Phase 5: fund manager (Config builds handle internally) ───────
        let fund_manager = FundManagerTask::new(Arc::clone(&config), Arc::clone(&snapshot_store));
        graph.add_task(fund_manager);

        // Set explicit start task (fan-out is first, but belt-and-suspenders).
        graph.set_start_task(TASK_ANALYST_FAN_OUT);

        graph
    }

    /// Run a full analysis cycle for the given initial state.
    ///
    /// 1. Pre-fetches shared news to avoid duplicate Finnhub calls.
    /// 2. Seeds a fresh in-memory session with the serialised `TradingState`.
    /// 3. Runs the `FlowRunner` loop until the pipeline completes.
    /// 4. Deserializes and returns the final `TradingState`.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError::GraphFlow`] on any graph-level error (including
    /// task failures that bubble out of the pipeline).
    #[instrument(skip(self, initial_state), fields(symbol = %initial_state.asset_symbol, date = %initial_state.target_date))]
    pub async fn run_analysis_cycle(
        &self,
        mut initial_state: TradingState,
    ) -> Result<TradingState, TradingError> {
        // Assign a fresh execution ID for this cycle.
        initial_state.execution_id = Uuid::new_v4();

        let symbol = initial_state.asset_symbol.clone();
        let date = initial_state.target_date.clone();
        let execution_id = initial_state.execution_id.to_string();
        info!(symbol = %symbol, date = %date, execution_id = %execution_id, "cycle started");

        // Fetch current market price from yfinance (best-effort — non-fatal if unavailable).
        if initial_state.current_price.is_none() {
            match self.yfinance.get_latest_close(&symbol, &date).await {
                Some(price) => {
                    info!(symbol = %symbol, price, "fetched current price from yfinance");
                    initial_state.current_price = Some(price);
                }
                None => {
                    info!(symbol = %symbol, "current price unavailable from yfinance");
                }
            }
        }

        // Pre-fetch shared news for Sentiment and News analysts (avoids duplicate Finnhub calls).
        let cached_news_json: Option<String> = {
            use crate::agents::analyst::prefetch_analyst_news;
            match prefetch_analyst_news(&self.finnhub, &initial_state.asset_symbol).await {
                Some(news_arc) => serde_json::to_string(news_arc.as_ref()).ok(),
                None => None,
            }
        };

        // ── Graph + storage ───────────────────────────────────────────────
        let graph = Arc::clone(&self.graph);
        let storage = Arc::new(InMemorySessionStorage::new());

        // ── Create session and seed context ───────────────────────────────
        let session_id = Uuid::new_v4().to_string();
        let session = Session::new_from_task(session_id.clone(), TASK_ANALYST_FAN_OUT);

        // Serialize trading state into context.
        serialize_state_to_context(&initial_state, &session.context)
            .await
            .map_err(|e| TradingError::GraphFlow {
                phase: "init".into(),
                task: "serialize_state".into(),
                cause: e.to_string(),
            })?;

        // Seed control counters.
        session
            .context
            .set(KEY_MAX_DEBATE_ROUNDS, self.config.llm.max_debate_rounds)
            .await;
        session
            .context
            .set(KEY_MAX_RISK_ROUNDS, self.config.llm.max_risk_rounds)
            .await;
        session.context.set(KEY_DEBATE_ROUND, 0u32).await;
        session.context.set(KEY_RISK_ROUND, 0u32).await;

        // Seed pre-fetched news into context so analysts can share it.
        if let Some(news_json) = cached_news_json {
            session.context.set(KEY_CACHED_NEWS, news_json).await;
        }

        // Persist seed session so FlowRunner can load it.
        storage
            .save(session)
            .await
            .map_err(|e| TradingError::GraphFlow {
                phase: "init".into(),
                task: "save_session".into(),
                cause: e.to_string(),
            })?;

        // ── Run pipeline to completion ────────────────────────────────────
        let runner = FlowRunner::new(graph, storage.clone());

        let mut step = 0usize;
        loop {
            step += 1;

            if step > MAX_PIPELINE_STEPS {
                return Err(TradingError::GraphFlow {
                    phase: "pipeline_execution".into(),
                    task: "step_ceiling".into(),
                    cause: format!(
                        "pipeline exceeded maximum of {MAX_PIPELINE_STEPS} steps — \
                         possible runaway loop from corrupted round counters or \
                         misconfigured conditional edges"
                    ),
                });
            }

            info!(step, "pipeline step");

            let result = runner.run(&session_id).await.map_err(map_graph_error)?;

            match result.status {
                ExecutionStatus::Completed => {
                    info!(steps = step, "pipeline completed");
                    break;
                }
                ExecutionStatus::Paused {
                    ref next_task_id, ..
                } => {
                    info!(next = %next_task_id, step, "pipeline paused, continuing");
                    // Continue looping — FlowRunner already saved progress.
                }
                ExecutionStatus::WaitingForInput => {
                    // Unexpected in this pipeline; treat as an error.
                    return Err(TradingError::GraphFlow {
                        phase: "pipeline_execution".into(),
                        task: "unexpected_input_wait".into(),
                        cause: "pipeline unexpectedly waiting for input".into(),
                    });
                }
                ExecutionStatus::Error(ref msg) => {
                    error!(error = %msg, step, "pipeline step returned error status");
                    // Parse the error message for task identity, same as GraphError path.
                    let (task_id, cause) = extract_task_identity(msg);
                    let phase = phase_for_task(&task_id);
                    let cause = sanitize_error_summary(&cause);
                    return Err(TradingError::GraphFlow {
                        phase,
                        task: task_id,
                        cause,
                    });
                }
            }
        }

        // ── Extract final TradingState from session context ───────────────
        let final_session = storage
            .get(&session_id)
            .await
            .map_err(|e| TradingError::GraphFlow {
                phase: "finalize".into(),
                task: "load_session".into(),
                cause: e.to_string(),
            })?
            .ok_or_else(|| TradingError::GraphFlow {
                phase: "finalize".into(),
                task: "load_session".into(),
                cause: format!("session '{session_id}' not found after completion"),
            })?;

        let final_state = deserialize_state_from_context(&final_session.context)
            .await
            .map_err(|e| TradingError::GraphFlow {
                phase: "finalize".into(),
                task: "deserialize_state".into(),
                cause: e.to_string(),
            })?;

        info!(symbol = %symbol, date = %date, execution_id = %execution_id, "cycle complete");
        Ok(final_state)
    }
}

// ── Error mapping ────────────────────────────────────────────────────────────

/// Map a [`graph_flow::GraphError`] into a [`TradingError::GraphFlow`],
/// preserving the real task identity and phase when available.
///
/// Graph-flow embeds task names in unstructured error strings like
/// `"Task 'bullish_researcher' failed: ..."`.  This function extracts the
/// task id from that pattern.  Our own task wrappers also prefix their error
/// messages with `"<TaskType>: ..."`.  The phase is inferred from the task id
/// using the known pipeline topology.
///
/// This is intentionally a free function (not a method) so it is independently
/// testable.
pub fn map_graph_error(err: graph_flow::GraphError) -> TradingError {
    match err {
        graph_flow::GraphError::TaskExecutionFailed(ref msg) => {
            let (task_id, cause) = extract_task_identity(msg);
            let phase = phase_for_task(&task_id);
            TradingError::GraphFlow {
                phase,
                task: task_id,
                cause: sanitize_error_summary(&cause),
            }
        }
        graph_flow::GraphError::TaskNotFound(ref id) => TradingError::GraphFlow {
            phase: phase_for_task(id),
            task: id.clone(),
            cause: format!("task not found: {id}"),
        },
        other => {
            // For session/storage/edge/context errors, use the variant name as
            // the task field so callers can distinguish error classes.
            let variant = match &other {
                graph_flow::GraphError::GraphNotFound(_) => "graph_not_found",
                graph_flow::GraphError::InvalidEdge(_) => "invalid_edge",
                graph_flow::GraphError::ContextError(_) => "context_error",
                graph_flow::GraphError::StorageError(_) => "storage_error",
                graph_flow::GraphError::SessionNotFound(_) => "session_not_found",
                _ => "graph_flow",
            };
            TradingError::GraphFlow {
                phase: "orchestration".into(),
                task: variant.into(),
                cause: sanitize_error_summary(&other.to_string()),
            }
        }
    }
}

/// Extract the task id from a graph-flow error message.
///
/// Graph-flow produces messages in the form `"Task '<id>' failed: <rest>"` and
/// `"FanOut child '<id>' failed: <rest>"`.  If neither pattern matches, the
/// full message is returned as the cause with `"unknown"` as the task id.
fn extract_task_identity(msg: &str) -> (String, String) {
    // Pattern 1: "Task '<task_id>' failed: <rest>"
    if let Some(rest) = msg.strip_prefix("Task '")
        && let Some(quote_end) = rest.find('\'')
    {
        let task_id = &rest[..quote_end];
        let cause = rest[quote_end..]
            .strip_prefix("' failed: ")
            .unwrap_or(&rest[quote_end..])
            .to_owned();
        return (task_id.to_owned(), cause);
    }

    // Pattern 2: "FanOut child '<task_id>' failed: <rest>"
    if let Some(rest) = msg.strip_prefix("FanOut child '")
        && let Some(quote_end) = rest.find('\'')
    {
        let task_id = &rest[..quote_end];
        let cause = rest[quote_end..]
            .strip_prefix("' failed: ")
            .unwrap_or(&rest[quote_end..])
            .to_owned();
        return (task_id.to_owned(), cause);
    }

    // Pattern 3: Our task wrappers prefix with "<TaskType>: <context>"
    // e.g. "BullishResearcherTask: failed to run bullish turn: ..."
    // In this case the full message IS the cause and the task id is unknown.
    ("unknown".to_owned(), msg.to_owned())
}

/// Map a graph task id to its pipeline phase name.
fn phase_for_task(task_id: &str) -> String {
    match task_id {
        "analyst_fanout"
        | "fundamental_analyst"
        | "sentiment_analyst"
        | "news_analyst"
        | "technical_analyst"
        | "analyst_sync" => "analyst_team".into(),
        "bullish_researcher" | "bearish_researcher" | "debate_moderator" => {
            "researcher_debate".into()
        }
        "trader" => "trader".into(),
        "aggressive_risk" | "conservative_risk" | "neutral_risk" | "risk_moderator" => {
            "risk_discussion".into()
        }
        "fund_manager" => "fund_manager".into(),
        _ => "unknown_phase".into(),
    }
}

#[cfg(any(test, feature = "test-helpers"))]
fn is_replaceable_task_id(task_id: &str) -> bool {
    REPLACEABLE_TASK_IDS.contains(&task_id)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_constants_match_task_impl_ids() {
        // These are the string literals used in add_edge / add_conditional_edge.
        // If any task's id() implementation changes, the graph wiring will silently
        // break.  This test catches that mismatch at compile-time.
        assert_eq!(TASK_ANALYST_FAN_OUT, "analyst_fanout");
        assert_eq!(TASK_ANALYST_SYNC, "analyst_sync");
        assert_eq!(TASK_BULLISH_RESEARCHER, "bullish_researcher");
        assert_eq!(TASK_BEARISH_RESEARCHER, "bearish_researcher");
        assert_eq!(TASK_DEBATE_MODERATOR, "debate_moderator");
        assert_eq!(TASK_TRADER, "trader");
        assert_eq!(TASK_AGGRESSIVE_RISK, "aggressive_risk");
        assert_eq!(TASK_CONSERVATIVE_RISK, "conservative_risk");
        assert_eq!(TASK_NEUTRAL_RISK, "neutral_risk");
        assert_eq!(TASK_RISK_MODERATOR, "risk_moderator");
        assert_eq!(TASK_FUND_MANAGER, "fund_manager");
    }
}
