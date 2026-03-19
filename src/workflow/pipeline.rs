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
use tracing::{error, info, instrument};

use crate::{
    config::Config,
    data::{FinnhubClient, YFinanceClient},
    error::TradingError,
    providers::factory::CompletionModelHandle,
    state::TradingState,
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        tasks::{
            AggressiveRiskTask, AnalystSyncTask, BearishResearcherTask, BullishResearcherTask,
            ConservativeRiskTask, DebateModeratorTask, FundManagerTask, FundamentalAnalystTask,
            KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND,
            NeutralRiskTask, NewsAnalystTask, RiskModeratorTask, SentimentAnalystTask,
            TechnicalAnalystTask,
        },
    },
};

// ── Graph task-ID constants ──────────────────────────────────────────────────

const TASK_ANALYST_FAN_OUT: &str = "analyst_fan_out";
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

// ── TradingPipeline ──────────────────────────────────────────────────────────

/// Orchestrates the full five-phase trading analysis pipeline.
///
/// Each call to [`run_analysis_cycle`][Self::run_analysis_cycle] builds a fresh
/// graph-flow `Graph`, runs it to completion, and returns the enriched
/// [`TradingState`].
pub struct TradingPipeline {
    config: Arc<Config>,
    finnhub: FinnhubClient,
    yfinance: YFinanceClient,
    snapshot_store: Arc<SnapshotStore>,
    /// Handle used by the deep-thinking agents (researcher, risk).
    handle: CompletionModelHandle,
}

impl TradingPipeline {
    /// Construct a new pipeline.
    ///
    /// # Parameters
    ///
    /// - `config` — application configuration (will be `Arc`-wrapped internally)
    /// - `finnhub` — Finnhub API client (used by analyst tasks)
    /// - `yfinance` — yfinance client (used by the technical analyst)
    /// - `snapshot_store` — SQLite-backed snapshot store for phase persistence
    /// - `handle` — pre-built completion-model handle for deep-thinking agents
    pub fn new(
        config: Config,
        finnhub: FinnhubClient,
        yfinance: YFinanceClient,
        snapshot_store: SnapshotStore,
        handle: CompletionModelHandle,
    ) -> Self {
        Self {
            config: Arc::new(config),
            finnhub,
            yfinance,
            snapshot_store: Arc::new(snapshot_store),
            handle,
        }
    }

    /// Build the directed [`Graph`] for one analysis cycle.
    ///
    /// The graph is stateless (all mutable state lives in the session
    /// [`Context`][graph_flow::Context]), so it is safe to share across
    /// concurrent cycles — but in practice we build a fresh one per cycle
    /// to avoid any retained state.
    pub fn build_graph(&self) -> Arc<Graph> {
        let graph = Arc::new(Graph::new("trading_pipeline"));

        // ── Phase 1: analyst fan-out ──────────────────────────────────────
        let fan_out = FanOutTask::new(
            TASK_ANALYST_FAN_OUT,
            vec![
                FundamentalAnalystTask::new(
                    self.handle.clone(),
                    self.finnhub.clone(),
                    self.config.llm.clone(),
                ),
                SentimentAnalystTask::new(
                    self.handle.clone(),
                    self.finnhub.clone(),
                    self.config.llm.clone(),
                ),
                NewsAnalystTask::new(
                    self.handle.clone(),
                    self.finnhub.clone(),
                    self.config.llm.clone(),
                ),
                TechnicalAnalystTask::new(
                    self.handle.clone(),
                    self.yfinance.clone(),
                    self.config.llm.clone(),
                ),
            ],
        );
        graph.add_task(fan_out);

        // ── Phase 1 sync: aggregation + degradation ───────────────────────
        let analyst_sync = AnalystSyncTask::new(Arc::clone(&self.snapshot_store));
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

        // ── Phase 2: researcher debate ────────────────────────────────────
        let bullish = BullishResearcherTask::new(Arc::clone(&self.config), self.handle.clone());
        let bearish = BearishResearcherTask::new();
        let debate_mod = DebateModeratorTask::new(Arc::clone(&self.snapshot_store));

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

        // ── Phase 3: trader ───────────────────────────────────────────────
        let trader = crate::workflow::tasks::TraderTask::new(
            Arc::clone(&self.config),
            Arc::clone(&self.snapshot_store),
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

        // ── Phase 4: risk discussion (sequential within each round) ───────
        let aggressive = AggressiveRiskTask::new(Arc::clone(&self.config), self.handle.clone());
        let conservative = ConservativeRiskTask::new();
        let neutral = NeutralRiskTask::new();
        let risk_mod = RiskModeratorTask::new(Arc::clone(&self.snapshot_store));

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

        // ── Phase 5: fund manager (terminal) ─────────────────────────────
        let fund_manager =
            FundManagerTask::new(Arc::clone(&self.config), Arc::clone(&self.snapshot_store));
        graph.add_task(fund_manager);

        // Set explicit start task (fan-out is first, but belt-and-suspenders).
        graph.set_start_task(TASK_ANALYST_FAN_OUT);

        graph
    }

    /// Run a full analysis cycle for the given initial state.
    ///
    /// 1. Builds the graph.
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
        initial_state: TradingState,
    ) -> Result<TradingState, TradingError> {
        let symbol = initial_state.asset_symbol.clone();
        let date = initial_state.target_date.clone();
        info!(symbol = %symbol, date = %date, "starting analysis cycle");

        // ── Build graph + storage ─────────────────────────────────────────
        let graph = self.build_graph();
        let storage = Arc::new(InMemorySessionStorage::new());

        // ── Create session and seed context ───────────────────────────────
        let session_id = uuid_session_id();
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
            info!(step, "pipeline step");

            let result = runner
                .run(&session_id)
                .await
                .map_err(|e| TradingError::GraphFlow {
                    phase: "execution".into(),
                    task: format!("step_{step}"),
                    cause: e.to_string(),
                })?;

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
                        phase: "execution".into(),
                        task: format!("step_{step}"),
                        cause: "pipeline unexpectedly waiting for input".into(),
                    });
                }
                ExecutionStatus::Error(ref msg) => {
                    error!(error = %msg, step, "pipeline step returned error status");
                    return Err(TradingError::GraphFlow {
                        phase: "execution".into(),
                        task: format!("step_{step}"),
                        cause: msg.clone(),
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

        info!(symbol = %symbol, date = %date, "analysis cycle complete");
        Ok(final_state)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Generate a short random session ID without pulling in a full UUID crate.
fn uuid_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("cycle-{}-{:08x}", std::process::id(), nanos)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_session_id_is_non_empty() {
        let id = uuid_session_id();
        assert!(!id.is_empty());
        assert!(id.starts_with("cycle-"));
    }

    #[test]
    fn task_id_constants_match_task_impl_ids() {
        // These are the string literals used in add_edge / add_conditional_edge.
        // If any task's id() implementation changes, the graph wiring will silently
        // break.  This test catches that mismatch at compile-time.
        assert_eq!(TASK_ANALYST_FAN_OUT, "analyst_fan_out");
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
