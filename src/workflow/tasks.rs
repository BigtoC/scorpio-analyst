//! Graph-flow [`Task`] wrappers for all five pipeline phases.
//!
//! Each struct in this module wraps an underlying agent call and translates
//! the result into graph-flow context mutations so the pipeline can advance
//! through its conditional edges.
//!
//! # Error policy for fan-out children (Phase 1)
//!
//! [`FundamentalAnalystTask`], [`SentimentAnalystTask`], [`NewsAnalystTask`],
//! and [`TechnicalAnalystTask`] run as children inside a [`FanOutTask`].
//! Because [`FanOutTask`] aborts the entire fan-out when **any** child returns
//! `Err`, these tasks **must never return `Err`**.  Instead they write:
//!
//! - On success: serialized analyst data under `"analyst.<type>"`, and
//!   `true` under `"analyst.<type>.ok"`.
//! - On failure: `false` under `"analyst.<type>.ok"` and the error message
//!   under `"analyst.<type>.err"`.
//!
//! [`AnalystSyncTask`] reads these flags, applies the degradation policy
//! (≥ 2 failures → `NextAction::End`), and merges successful results into
//! the main `TradingState`.

use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::{error, info, warn};

use crate::{
    agents::{
        analyst::{FundamentalAnalyst, NewsAnalyst, SentimentAnalyst, TechnicalAnalyst},
        fund_manager::run_fund_manager,
        researcher::run_researcher_debate,
        risk::run_risk_discussion,
        trader::run_trader,
    },
    config::{Config, LlmConfig},
    data::{FinnhubClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    state::AgentTokenUsage,
    workflow::{
        context_bridge::{
            deserialize_state_from_context, read_prefixed_result, serialize_state_to_context,
            write_prefixed_result,
        },
        snapshot::SnapshotStore,
    },
};

// ────────────────────────────────────────────────────────────────────────────
// Context key constants
// ────────────────────────────────────────────────────────────────────────────

/// Context key for the maximum number of researcher debate rounds.
pub const KEY_MAX_DEBATE_ROUNDS: &str = "max_debate_rounds";
/// Context key for the maximum number of risk discussion rounds.
pub const KEY_MAX_RISK_ROUNDS: &str = "max_risk_rounds";
/// Context key for the current researcher debate round counter.
pub const KEY_DEBATE_ROUND: &str = "debate_round";
/// Context key for the current risk discussion round counter.
pub const KEY_RISK_ROUND: &str = "risk_round";

/// Analyst context key prefix (e.g. `"analyst.fundamental"`).
const ANALYST_PREFIX: &str = "analyst";

/// Suffix used for analyst success flag keys (e.g. `"analyst.fundamental.ok"`).
const OK_SUFFIX: &str = "ok";
/// Suffix used for analyst error message keys (e.g. `"analyst.fundamental.err"`).
const ERR_SUFFIX: &str = "err";

/// Names and context sub-key identifiers for the four analysts.
const ANALYST_FUNDAMENTAL: &str = "fundamental";
const ANALYST_SENTIMENT: &str = "sentiment";
const ANALYST_NEWS: &str = "news";
const ANALYST_TECHNICAL: &str = "technical";

// ────────────────────────────────────────────────────────────────────────────
// Phase 1 — Analyst Fan-Out child tasks
// ────────────────────────────────────────────────────────────────────────────

/// Graph-flow task wrapper for the Fundamental Analyst.
///
/// Runs inside a [`FanOutTask`][graph_flow::FanOutTask] alongside the other
/// three analyst tasks.  Errors are captured into context flags rather than
/// returned, so the [`FanOutTask`] never aborts on a single analyst failure.
pub struct FundamentalAnalystTask {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    llm_config: LlmConfig,
}

impl FundamentalAnalystTask {
    /// Create a new `FundamentalAnalystTask`.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        llm_config: LlmConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle,
            finnhub,
            llm_config,
        })
    }
}

#[async_trait]
impl Task for FundamentalAnalystTask {
    fn id(&self) -> &str {
        "fundamental_analyst"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let state = match deserialize_state_from_context(&context).await {
            Ok(s) => s,
            Err(e) => {
                error!(analyst = "fundamental", error = %e, "failed to deserialize state");
                write_flag(&context, ANALYST_FUNDAMENTAL, false).await;
                write_err(&context, ANALYST_FUNDAMENTAL, &e.to_string()).await;
                return Ok(TaskResult::new(None, NextAction::Continue));
            }
        };

        let analyst = FundamentalAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
        );

        match analyst.run().await {
            Ok((data, _usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_FUNDAMENTAL, &data)
                        .await
                {
                    error!(analyst = "fundamental", error = %e, "failed to write result to context");
                    write_flag(&context, ANALYST_FUNDAMENTAL, false).await;
                    write_err(&context, ANALYST_FUNDAMENTAL, &e.to_string()).await;
                } else {
                    write_flag(&context, ANALYST_FUNDAMENTAL, true).await;
                    info!(analyst = "fundamental", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "fundamental", error = %e, "analyst failed");
                write_flag(&context, ANALYST_FUNDAMENTAL, false).await;
                write_err(&context, ANALYST_FUNDAMENTAL, &e.to_string()).await;
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Graph-flow task wrapper for the Sentiment Analyst.
pub struct SentimentAnalystTask {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    llm_config: LlmConfig,
}

impl SentimentAnalystTask {
    /// Create a new `SentimentAnalystTask`.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        llm_config: LlmConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle,
            finnhub,
            llm_config,
        })
    }
}

#[async_trait]
impl Task for SentimentAnalystTask {
    fn id(&self) -> &str {
        "sentiment_analyst"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let state = match deserialize_state_from_context(&context).await {
            Ok(s) => s,
            Err(e) => {
                error!(analyst = "sentiment", error = %e, "failed to deserialize state");
                write_flag(&context, ANALYST_SENTIMENT, false).await;
                write_err(&context, ANALYST_SENTIMENT, &e.to_string()).await;
                return Ok(TaskResult::new(None, NextAction::Continue));
            }
        };

        let analyst = SentimentAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
            None, // no pre-fetched news cache available in task context
        );

        match analyst.run().await {
            Ok((data, _usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_SENTIMENT, &data).await
                {
                    error!(analyst = "sentiment", error = %e, "failed to write result to context");
                    write_flag(&context, ANALYST_SENTIMENT, false).await;
                    write_err(&context, ANALYST_SENTIMENT, &e.to_string()).await;
                } else {
                    write_flag(&context, ANALYST_SENTIMENT, true).await;
                    info!(analyst = "sentiment", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "sentiment", error = %e, "analyst failed");
                write_flag(&context, ANALYST_SENTIMENT, false).await;
                write_err(&context, ANALYST_SENTIMENT, &e.to_string()).await;
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Graph-flow task wrapper for the News Analyst.
pub struct NewsAnalystTask {
    handle: CompletionModelHandle,
    finnhub: FinnhubClient,
    llm_config: LlmConfig,
}

impl NewsAnalystTask {
    /// Create a new `NewsAnalystTask`.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        llm_config: LlmConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle,
            finnhub,
            llm_config,
        })
    }
}

#[async_trait]
impl Task for NewsAnalystTask {
    fn id(&self) -> &str {
        "news_analyst"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let state = match deserialize_state_from_context(&context).await {
            Ok(s) => s,
            Err(e) => {
                error!(analyst = "news", error = %e, "failed to deserialize state");
                write_flag(&context, ANALYST_NEWS, false).await;
                write_err(&context, ANALYST_NEWS, &e.to_string()).await;
                return Ok(TaskResult::new(None, NextAction::Continue));
            }
        };

        let analyst = NewsAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
            None, // no pre-fetched news cache available in task context
        );

        match analyst.run().await {
            Ok((data, _usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_NEWS, &data).await
                {
                    error!(analyst = "news", error = %e, "failed to write result to context");
                    write_flag(&context, ANALYST_NEWS, false).await;
                    write_err(&context, ANALYST_NEWS, &e.to_string()).await;
                } else {
                    write_flag(&context, ANALYST_NEWS, true).await;
                    info!(analyst = "news", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "news", error = %e, "analyst failed");
                write_flag(&context, ANALYST_NEWS, false).await;
                write_err(&context, ANALYST_NEWS, &e.to_string()).await;
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Graph-flow task wrapper for the Technical Analyst.
pub struct TechnicalAnalystTask {
    handle: CompletionModelHandle,
    yfinance: YFinanceClient,
    llm_config: LlmConfig,
}

impl TechnicalAnalystTask {
    /// Create a new `TechnicalAnalystTask`.
    pub fn new(
        handle: CompletionModelHandle,
        yfinance: YFinanceClient,
        llm_config: LlmConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle,
            yfinance,
            llm_config,
        })
    }
}

#[async_trait]
impl Task for TechnicalAnalystTask {
    fn id(&self) -> &str {
        "technical_analyst"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let state = match deserialize_state_from_context(&context).await {
            Ok(s) => s,
            Err(e) => {
                error!(analyst = "technical", error = %e, "failed to deserialize state");
                write_flag(&context, ANALYST_TECHNICAL, false).await;
                write_err(&context, ANALYST_TECHNICAL, &e.to_string()).await;
                return Ok(TaskResult::new(None, NextAction::Continue));
            }
        };

        let analyst = TechnicalAnalyst::new(
            self.handle.clone(),
            self.yfinance.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
        );

        match analyst.run().await {
            Ok((data, _usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_TECHNICAL, &data).await
                {
                    error!(analyst = "technical", error = %e, "failed to write result to context");
                    write_flag(&context, ANALYST_TECHNICAL, false).await;
                    write_err(&context, ANALYST_TECHNICAL, &e.to_string()).await;
                } else {
                    write_flag(&context, ANALYST_TECHNICAL, true).await;
                    info!(analyst = "technical", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "technical", error = %e, "analyst failed");
                write_flag(&context, ANALYST_TECHNICAL, false).await;
                write_err(&context, ANALYST_TECHNICAL, &e.to_string()).await;
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 1 — Analyst sync / aggregation task
// ────────────────────────────────────────────────────────────────────────────

/// Reads all four analyst results from context, merges them into [`TradingState`],
/// applies the degradation policy, and saves a phase snapshot.
///
/// This task runs **after** the [`FanOutTask`][graph_flow::FanOutTask] containing
/// the four analyst child tasks.
///
/// # Degradation policy
///
/// - 0–1 failures → continue with partial data
/// - 2+ failures  → return [`NextAction::End`] to abort the pipeline
pub struct AnalystSyncTask {
    snapshot_store: Arc<SnapshotStore>,
}

impl AnalystSyncTask {
    /// Create a new `AnalystSyncTask`.
    pub fn new(snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self { snapshot_store })
    }
}

#[async_trait]
impl Task for AnalystSyncTask {
    fn id(&self) -> &str {
        "analyst_sync"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to deserialize state: {e}"
                ))
            })?;

        // ── Collect results and failure counts ──────────────────────────────
        let mut failures: Vec<&str> = Vec::new();
        let mut token_usages: Vec<AgentTokenUsage> = Vec::new();

        // Use context.get (async) for reading analyst flags properly
        let key_fund_ok = format!("{ANALYST_PREFIX}.{ANALYST_FUNDAMENTAL}.{OK_SUFFIX}");
        let key_sent_ok = format!("{ANALYST_PREFIX}.{ANALYST_SENTIMENT}.{OK_SUFFIX}");
        let key_news_ok = format!("{ANALYST_PREFIX}.{ANALYST_NEWS}.{OK_SUFFIX}");
        let key_tech_ok = format!("{ANALYST_PREFIX}.{ANALYST_TECHNICAL}.{OK_SUFFIX}");
        let fund_ok: bool = context.get(&key_fund_ok).await.unwrap_or(false);
        let sent_ok: bool = context.get(&key_sent_ok).await.unwrap_or(false);
        let news_ok: bool = context.get(&key_news_ok).await.unwrap_or(false);
        let tech_ok: bool = context.get(&key_tech_ok).await.unwrap_or(false);

        // Fundamental
        if fund_ok {
            match read_prefixed_result::<crate::state::FundamentalData>(
                &context,
                ANALYST_PREFIX,
                ANALYST_FUNDAMENTAL,
            )
            .await
            {
                Ok(data) => {
                    state.fundamental_metrics = Some(data);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Fundamental Analyst",
                        "unknown",
                        0,
                    ));
                }
                Err(e) => {
                    warn!(analyst = "fundamental", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_FUNDAMENTAL);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Fundamental Analyst",
                        "unknown",
                        0,
                    ));
                }
            }
        } else {
            failures.push(ANALYST_FUNDAMENTAL);
            token_usages.push(AgentTokenUsage::unavailable(
                "Fundamental Analyst",
                "unknown",
                0,
            ));
        }

        // Sentiment
        if sent_ok {
            match read_prefixed_result::<crate::state::SentimentData>(
                &context,
                ANALYST_PREFIX,
                ANALYST_SENTIMENT,
            )
            .await
            {
                Ok(data) => {
                    state.market_sentiment = Some(data);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Sentiment Analyst",
                        "unknown",
                        0,
                    ));
                }
                Err(e) => {
                    warn!(analyst = "sentiment", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_SENTIMENT);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Sentiment Analyst",
                        "unknown",
                        0,
                    ));
                }
            }
        } else {
            failures.push(ANALYST_SENTIMENT);
            token_usages.push(AgentTokenUsage::unavailable(
                "Sentiment Analyst",
                "unknown",
                0,
            ));
        }

        // News
        if news_ok {
            match read_prefixed_result::<crate::state::NewsData>(
                &context,
                ANALYST_PREFIX,
                ANALYST_NEWS,
            )
            .await
            {
                Ok(data) => {
                    state.macro_news = Some(data);
                    token_usages.push(AgentTokenUsage::unavailable("News Analyst", "unknown", 0));
                }
                Err(e) => {
                    warn!(analyst = "news", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_NEWS);
                    token_usages.push(AgentTokenUsage::unavailable("News Analyst", "unknown", 0));
                }
            }
        } else {
            failures.push(ANALYST_NEWS);
            token_usages.push(AgentTokenUsage::unavailable("News Analyst", "unknown", 0));
        }

        // Technical
        if tech_ok {
            match read_prefixed_result::<crate::state::TechnicalData>(
                &context,
                ANALYST_PREFIX,
                ANALYST_TECHNICAL,
            )
            .await
            {
                Ok(data) => {
                    state.technical_indicators = Some(data);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Technical Analyst",
                        "unknown",
                        0,
                    ));
                }
                Err(e) => {
                    warn!(analyst = "technical", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_TECHNICAL);
                    token_usages.push(AgentTokenUsage::unavailable(
                        "Technical Analyst",
                        "unknown",
                        0,
                    ));
                }
            }
        } else {
            failures.push(ANALYST_TECHNICAL);
            token_usages.push(AgentTokenUsage::unavailable(
                "Technical Analyst",
                "unknown",
                0,
            ));
        }

        // ── Degradation check ───────────────────────────────────────────────
        let failure_count = failures.len();
        if failure_count >= 2 {
            error!(
                failures = ?failures,
                "AnalystSyncTask: {}/{} analysts failed — aborting pipeline",
                failure_count, 4
            );
            return Ok(TaskResult::new(
                Some(format!(
                    "{failure_count}/4 analysts failed — pipeline aborted"
                )),
                NextAction::End,
            ));
        }

        // ── Re-serialize updated state ──────────────────────────────────────
        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to serialize state: {e}"
                ))
            })?;

        // ── Save phase snapshot ─────────────────────────────────────────────
        let execution_id = state.execution_id.to_string();
        if let Err(e) = self
            .snapshot_store
            .save_snapshot(
                &execution_id,
                1,
                "analyst_team",
                &state,
                Some(&token_usages),
            )
            .await
        {
            warn!(error = %e, "failed to save phase 1 snapshot — continuing");
        }

        info!(
            failures = failure_count,
            "AnalystSyncTask: phase 1 complete"
        );

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 2 — Researcher Debate tasks
// ────────────────────────────────────────────────────────────────────────────

/// Runs one turn of the Bullish Researcher and increments `"debate_round"` in context.
pub struct BullishResearcherTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl BullishResearcherTask {
    /// Create a new `BullishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BullishResearcherTask {
    fn id(&self) -> &str {
        "bullish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to deserialize state: {e}"
                ))
            })?;

        // Run one full debate round (bull + bear + moderator is handled by separate tasks).
        // Here we only need to call the bull turn; the loop structure is handled by the graph.
        // However, run_researcher_debate runs the full loop — so we need a single-turn approach.
        // Instead, we'll call run_researcher_debate with max_rounds=1 each time we pass through here.
        // The debate_round counter in context tracks cumulative rounds.

        // Override max_debate_rounds to 1 so we run exactly one round here.
        let mut single_round_config = (*self.config).clone();
        single_round_config.llm.max_debate_rounds = 1;

        match run_researcher_debate(&mut state, &single_round_config, &self.handle).await {
            Ok(_usages) => {
                // Increment debate_round counter in context
                let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
                context.set(KEY_DEBATE_ROUND, current_round + 1).await;

                serialize_state_to_context(&state, &context)
                    .await
                    .map_err(|e| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "BullishResearcherTask: failed to serialize state: {e}"
                        ))
                    })?;

                info!(
                    round = current_round + 1,
                    "BullishResearcherTask: debate round complete"
                );
            }
            Err(e) => {
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: researcher debate failed: {e}"
                )));
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Bearish Researcher turn (part of a debate cycle).
///
/// Note: Since [`run_researcher_debate`] runs the full loop (bull+bear+moderator),
/// this task is a no-op placeholder — the full round is performed by
/// [`BullishResearcherTask`].  The pipeline wires Bullish → Bearish → Moderator
/// to match the spec node topology, but the actual LLM work is done in
/// `BullishResearcherTask` for simplicity.
pub struct BearishResearcherTask;

impl BearishResearcherTask {
    /// Create a new `BearishResearcherTask`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Default for BearishResearcherTask {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Task for BearishResearcherTask {
    fn id(&self) -> &str {
        "bearish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // The actual bear turn was performed inside BullishResearcherTask.
        // This task exists to satisfy graph topology requirements from the spec.
        let _: Option<u32> = context.get(KEY_DEBATE_ROUND).await;
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Debate Moderator (produces consensus summary).
///
/// On the final debate round it saves a phase snapshot.
pub struct DebateModeratorTask {
    snapshot_store: Arc<SnapshotStore>,
}

impl DebateModeratorTask {
    /// Create a new `DebateModeratorTask`.
    pub fn new(snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self { snapshot_store })
    }
}

#[async_trait]
impl Task for DebateModeratorTask {
    fn id(&self) -> &str {
        "debate_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // The moderator consensus was already written to state by BullishResearcherTask
        // (run_researcher_debate includes the moderator call).
        // We just need to read the current state and save a snapshot on the final round.
        let state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to deserialize state: {e}"
                ))
            })?;

        let debate_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let max_rounds: u32 = context.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);

        // Save snapshot when the debate is complete (last round or zero-round case).
        if debate_round >= max_rounds {
            let execution_id = state.execution_id.to_string();
            if let Err(e) = self
                .snapshot_store
                .save_snapshot(&execution_id, 2, "researcher_debate", &state, None)
                .await
            {
                warn!(error = %e, "failed to save phase 2 snapshot — continuing");
            }
            info!(
                rounds = debate_round,
                "DebateModeratorTask: debate complete, snapshot saved"
            );
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 3 — Trader task
// ────────────────────────────────────────────────────────────────────────────

/// Synthesizes analyst outputs and debate consensus into a [`TradeProposal`].
pub struct TraderTask {
    config: Arc<Config>,
    snapshot_store: Arc<SnapshotStore>,
}

impl TraderTask {
    /// Create a new `TraderTask`.
    pub fn new(config: Arc<Config>, snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self {
            config,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for TraderTask {
    fn id(&self) -> &str {
        "trader"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to deserialize state: {e}"
                ))
            })?;

        let usage = run_trader(&mut state, &self.config).await.map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "TraderTask: run_trader failed: {e}"
            ))
        })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to serialize state: {e}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        if let Err(e) = self
            .snapshot_store
            .save_snapshot(&execution_id, 3, "trader", &state, Some(&[usage]))
            .await
        {
            warn!(error = %e, "failed to save phase 3 snapshot — continuing");
        }

        info!("TraderTask: trade proposal generated");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 4 — Risk Discussion tasks (sequential)
// ────────────────────────────────────────────────────────────────────────────

/// Runs one round of the Aggressive Risk agent and increments `"risk_round"`.
///
/// Runs the full round (Aggressive → Conservative → Neutral) via
/// [`run_risk_discussion`] with `max_rounds=1` — similar to the debate approach.
pub struct AggressiveRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl AggressiveRiskTask {
    /// Create a new `AggressiveRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for AggressiveRiskTask {
    fn id(&self) -> &str {
        "aggressive_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AggressiveRiskTask: failed to deserialize state: {e}"
                ))
            })?;

        // Run exactly one risk discussion round.
        let mut single_round_config = (*self.config).clone();
        single_round_config.llm.max_risk_rounds = 1;

        match run_risk_discussion(&mut state, &single_round_config, &self.handle).await {
            Ok(_usages) => {
                let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
                context.set(KEY_RISK_ROUND, current_round + 1).await;

                serialize_state_to_context(&state, &context)
                    .await
                    .map_err(|e| {
                        graph_flow::GraphError::TaskExecutionFailed(format!(
                            "AggressiveRiskTask: failed to serialize state: {e}"
                        ))
                    })?;

                info!(
                    round = current_round + 1,
                    "AggressiveRiskTask: risk round complete"
                );
            }
            Err(e) => {
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AggressiveRiskTask: run_risk_discussion failed: {e}"
                )));
            }
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Conservative Risk agent turn (part of a risk round).
///
/// Like [`BearishResearcherTask`], this is a topology placeholder — the actual
/// round is executed in [`AggressiveRiskTask`] via [`run_risk_discussion`].
pub struct ConservativeRiskTask;

impl ConservativeRiskTask {
    /// Create a new `ConservativeRiskTask`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Default for ConservativeRiskTask {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Task for ConservativeRiskTask {
    fn id(&self) -> &str {
        "conservative_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let _: Option<u32> = context.get(KEY_RISK_ROUND).await;
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Neutral Risk agent turn (part of a risk round).
pub struct NeutralRiskTask;

impl NeutralRiskTask {
    /// Create a new `NeutralRiskTask`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl Default for NeutralRiskTask {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl Task for NeutralRiskTask {
    fn id(&self) -> &str {
        "neutral_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let _: Option<u32> = context.get(KEY_RISK_ROUND).await;
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Risk Moderator (synthesizes risk perspectives).
///
/// On the final risk round it saves a phase snapshot.
pub struct RiskModeratorTask {
    snapshot_store: Arc<SnapshotStore>,
}

impl RiskModeratorTask {
    /// Create a new `RiskModeratorTask`.
    pub fn new(snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self { snapshot_store })
    }
}

#[async_trait]
impl Task for RiskModeratorTask {
    fn id(&self) -> &str {
        "risk_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        // Risk moderation was performed inside AggressiveRiskTask (full round).
        let state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "RiskModeratorTask: failed to deserialize state: {e}"
                ))
            })?;

        let risk_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let max_rounds: u32 = context.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);

        if risk_round >= max_rounds {
            let execution_id = state.execution_id.to_string();
            if let Err(e) = self
                .snapshot_store
                .save_snapshot(&execution_id, 4, "risk_discussion", &state, None)
                .await
            {
                warn!(error = %e, "failed to save phase 4 snapshot — continuing");
            }
            info!(
                rounds = risk_round,
                "RiskModeratorTask: risk discussion complete, snapshot saved"
            );
        }

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 5 — Fund Manager task
// ────────────────────────────────────────────────────────────────────────────

/// Makes the final approve/reject decision and terminates the pipeline.
pub struct FundManagerTask {
    config: Arc<Config>,
    snapshot_store: Arc<SnapshotStore>,
}

impl FundManagerTask {
    /// Create a new `FundManagerTask`.
    pub fn new(config: Arc<Config>, snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self {
            config,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for FundManagerTask {
    fn id(&self) -> &str {
        "fund_manager"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: failed to deserialize state: {e}"
                ))
            })?;

        let usage = run_fund_manager(&mut state, &self.config)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: run_fund_manager failed: {e}"
                ))
            })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: failed to serialize state: {e}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        if let Err(e) = self
            .snapshot_store
            .save_snapshot(&execution_id, 5, "fund_manager", &state, Some(&[usage]))
            .await
        {
            warn!(error = %e, "failed to save phase 5 snapshot — continuing");
        }

        info!(
            decision = ?state.final_execution_status,
            "FundManagerTask: pipeline complete"
        );

        Ok(TaskResult::new(None, NextAction::End))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Write an analyst success/failure boolean flag to context.
async fn write_flag(context: &Context, analyst_key: &str, ok: bool) {
    context
        .set(format!("{ANALYST_PREFIX}.{analyst_key}.{OK_SUFFIX}"), ok)
        .await;
}

/// Write an analyst error message to context.
async fn write_err(context: &Context, analyst_key: &str, message: &str) {
    context
        .set(
            format!("{ANALYST_PREFIX}.{analyst_key}.{ERR_SUFFIX}"),
            message.to_owned(),
        )
        .await;
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TradingState;
    use graph_flow::Context;

    // ── Helper ───────────────────────────────────────────────────────────

    fn sample_state() -> TradingState {
        TradingState::new("AAPL", "2026-03-19")
    }

    async fn seed_state(ctx: &Context, state: &TradingState) {
        serialize_state_to_context(state, ctx)
            .await
            .expect("seed state serialization should succeed");
    }

    // ── Analyst flag helpers ─────────────────────────────────────────────

    #[tokio::test]
    async fn write_flag_true_readable_back() {
        let ctx = Context::new();
        write_flag(&ctx, ANALYST_FUNDAMENTAL, true).await;
        let key = format!("{ANALYST_PREFIX}.{ANALYST_FUNDAMENTAL}.{OK_SUFFIX}");
        let ok: Option<bool> = ctx.get(&key).await;
        assert_eq!(ok, Some(true));
    }

    #[tokio::test]
    async fn write_flag_false_readable_back() {
        let ctx = Context::new();
        write_flag(&ctx, ANALYST_SENTIMENT, false).await;
        let key = format!("{ANALYST_PREFIX}.{ANALYST_SENTIMENT}.{OK_SUFFIX}");
        let ok: Option<bool> = ctx.get(&key).await;
        assert_eq!(ok, Some(false));
    }

    #[tokio::test]
    async fn write_err_readable_back() {
        let ctx = Context::new();
        write_err(&ctx, ANALYST_NEWS, "something went wrong").await;
        let key = format!("{ANALYST_PREFIX}.{ANALYST_NEWS}.{ERR_SUFFIX}");
        let msg: Option<String> = ctx.get(&key).await;
        assert_eq!(msg.as_deref(), Some("something went wrong"));
    }

    // ── AnalystSyncTask: all 4 succeed ───────────────────────────────────

    #[tokio::test]
    async fn analyst_sync_all_succeed_returns_continue() {
        use crate::state::{FundamentalData, NewsData, SentimentData, TechnicalData};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store creation should succeed"),
        );

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;

        // Write all 4 analyst results as successful
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_FUNDAMENTAL}.{OK_SUFFIX}"),
            true,
        )
        .await;
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_SENTIMENT}.{OK_SUFFIX}"),
            true,
        )
        .await;
        ctx.set(format!("{ANALYST_PREFIX}.{ANALYST_NEWS}.{OK_SUFFIX}"), true)
            .await;
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_TECHNICAL}.{OK_SUFFIX}"),
            true,
        )
        .await;

        // Write minimal analyst data
        let fund = FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_FUNDAMENTAL, &fund)
            .await
            .unwrap();

        let sent = SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_SENTIMENT, &sent)
            .await
            .unwrap();

        let news = NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_NEWS, &news)
            .await
            .unwrap();

        let tech = TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_TECHNICAL, &tech)
            .await
            .unwrap();

        let task = AnalystSyncTask::new(store);
        let result = task.run(ctx.clone()).await.expect("task should succeed");

        assert_eq!(result.next_action, NextAction::Continue);

        // Verify state was re-serialized with analyst data merged
        let recovered = deserialize_state_from_context(&ctx).await.unwrap();
        assert!(recovered.fundamental_metrics.is_some());
        assert!(recovered.market_sentiment.is_some());
        assert!(recovered.macro_news.is_some());
        assert!(recovered.technical_indicators.is_some());
    }

    // ── AnalystSyncTask: 1 failure → continues ───────────────────────────

    #[tokio::test]
    async fn analyst_sync_one_failure_continues() {
        use crate::state::{NewsData, SentimentData, TechnicalData};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store creation should succeed"),
        );

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;

        // Fundamental fails
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_FUNDAMENTAL}.{OK_SUFFIX}"),
            false,
        )
        .await;
        write_err(&ctx, ANALYST_FUNDAMENTAL, "simulated failure").await;

        // Others succeed
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_SENTIMENT}.{OK_SUFFIX}"),
            true,
        )
        .await;
        ctx.set(format!("{ANALYST_PREFIX}.{ANALYST_NEWS}.{OK_SUFFIX}"), true)
            .await;
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_TECHNICAL}.{OK_SUFFIX}"),
            true,
        )
        .await;

        let sent = SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_SENTIMENT, &sent)
            .await
            .unwrap();

        let news = NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_NEWS, &news)
            .await
            .unwrap();

        let tech = TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_TECHNICAL, &tech)
            .await
            .unwrap();

        let task = AnalystSyncTask::new(store);
        let result = task.run(ctx.clone()).await.expect("task should succeed");

        // 1 failure should continue
        assert_eq!(result.next_action, NextAction::Continue);

        let recovered = deserialize_state_from_context(&ctx).await.unwrap();
        assert!(
            recovered.fundamental_metrics.is_none(),
            "failed analyst field should be None"
        );
        assert!(recovered.market_sentiment.is_some());
        assert!(recovered.macro_news.is_some());
        assert!(recovered.technical_indicators.is_some());
    }

    // ── AnalystSyncTask: 2 failures → aborts ─────────────────────────────

    #[tokio::test]
    async fn analyst_sync_two_failures_aborts() {
        use crate::state::{NewsData, TechnicalData};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store creation should succeed"),
        );

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;

        // Fundamental and Sentiment fail
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_FUNDAMENTAL}.{OK_SUFFIX}"),
            false,
        )
        .await;
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_SENTIMENT}.{OK_SUFFIX}"),
            false,
        )
        .await;
        write_err(&ctx, ANALYST_FUNDAMENTAL, "error 1").await;
        write_err(&ctx, ANALYST_SENTIMENT, "error 2").await;

        // Others succeed
        ctx.set(format!("{ANALYST_PREFIX}.{ANALYST_NEWS}.{OK_SUFFIX}"), true)
            .await;
        ctx.set(
            format!("{ANALYST_PREFIX}.{ANALYST_TECHNICAL}.{OK_SUFFIX}"),
            true,
        )
        .await;

        let news = NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_NEWS, &news)
            .await
            .unwrap();

        let tech = TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "ok".to_owned(),
        };
        write_prefixed_result(&ctx, ANALYST_PREFIX, ANALYST_TECHNICAL, &tech)
            .await
            .unwrap();

        let task = AnalystSyncTask::new(store);
        let result = task
            .run(ctx.clone())
            .await
            .expect("task itself should not error");

        // 2 failures should return End
        assert_eq!(result.next_action, NextAction::End);
    }

    // ── BearishResearcherTask: no-op, returns Continue ───────────────────

    #[tokio::test]
    async fn bearish_researcher_task_is_noop() {
        let ctx = Context::new();
        ctx.set(KEY_DEBATE_ROUND, 1u32).await;

        let task = BearishResearcherTask::new();
        let result = task.run(ctx.clone()).await.expect("task should succeed");
        assert_eq!(result.next_action, NextAction::Continue);
    }

    // ── ConservativeRiskTask: no-op, returns Continue ────────────────────

    #[tokio::test]
    async fn conservative_risk_task_is_noop() {
        let ctx = Context::new();
        ctx.set(KEY_RISK_ROUND, 1u32).await;

        let task = ConservativeRiskTask::new();
        let result = task.run(ctx.clone()).await.expect("task should succeed");
        assert_eq!(result.next_action, NextAction::Continue);
    }

    // ── NeutralRiskTask: no-op, returns Continue ─────────────────────────

    #[tokio::test]
    async fn neutral_risk_task_is_noop() {
        let ctx = Context::new();
        ctx.set(KEY_RISK_ROUND, 0u32).await;

        let task = NeutralRiskTask::new();
        let result = task.run(ctx.clone()).await.expect("task should succeed");
        assert_eq!(result.next_action, NextAction::Continue);
    }

    // ── Task IDs match expected graph node names ─────────────────────────

    #[test]
    fn task_ids_are_correct() {
        // Verify static task IDs using the no-arg constructors for no-op tasks.
        assert_eq!(BearishResearcherTask::new().id(), "bearish_researcher");
        assert_eq!(ConservativeRiskTask::new().id(), "conservative_risk");
        assert_eq!(NeutralRiskTask::new().id(), "neutral_risk");
    }
}
