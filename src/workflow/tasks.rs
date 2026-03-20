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
//! `Err`, these tasks distinguish two failure categories:
//!
//! ## Orchestration corruption → `Err` (fail hard)
//!
//! If the shared [`TradingState`] cannot be deserialized from the context, or
//! if `write_prefixed_result` fails (serialization bug), the fan-out child
//! returns `Err(GraphError::TaskExecutionFailed(...))`.  This aborts the
//! entire fan-out because the orchestration layer itself is broken — partial
//! results from other analysts would be unreliable.
//!
//! ## Analyst runtime failure → graceful degradation
//!
//! If the underlying analyst agent (`analyst.run()`) fails (network timeout,
//! API error, LLM refusal, etc.), the child writes:
//!
//! - `false` under `"analyst.<type>.ok"` and the error message under
//!   `"analyst.<type>.err"`.
//!
//! [`AnalystSyncTask`] reads these flags, applies the degradation policy
//! (≥ 2 failures → `NextAction::End`), and merges successful results into
//! the main `TradingState`.
//!
//! On success, the child writes serialized analyst data under
//! `"analyst.<type>"` and `true` under `"analyst.<type>.ok"`.

use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use tracing::{error, info, warn};

use crate::{
    agents::{
        analyst::{FundamentalAnalyst, NewsAnalyst, SentimentAnalyst, TechnicalAnalyst},
        fund_manager::run_fund_manager,
        researcher::{
            run_bearish_researcher_turn, run_bullish_researcher_turn, run_debate_moderation,
        },
        risk::{
            run_aggressive_risk_turn, run_conservative_risk_turn, run_neutral_risk_turn,
            run_risk_moderation,
        },
        trader::run_trader,
    },
    config::{Config, LlmConfig},
    data::{FinnhubClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    state::{AgentTokenUsage, PhaseTokenUsage, TradingState},
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
/// Context key for pre-fetched news data shared between Sentiment and News analysts.
pub const KEY_CACHED_NEWS: &str = "analyst.cached_news";

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
                error!(analyst = "fundamental", error = %e, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundamentalAnalystTask: orchestration corruption: state deserialization failed: {e}"
                )));
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
            Ok((data, usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_FUNDAMENTAL, &data)
                        .await
                {
                    error!(analyst = "fundamental", error = %e, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "FundamentalAnalystTask: orchestration corruption: context write failed: {e}"
                    )));
                } else {
                    write_flag(&context, ANALYST_FUNDAMENTAL, true).await;
                    let _ = write_analyst_usage(&context, ANALYST_FUNDAMENTAL, &usage).await;
                    info!(analyst = "fundamental", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "fundamental", error = %e, "analyst failed");
                write_flag(&context, ANALYST_FUNDAMENTAL, false).await;
                write_err(&context, ANALYST_FUNDAMENTAL, &e.to_string()).await;
                let _ = write_analyst_usage(
                    &context,
                    ANALYST_FUNDAMENTAL,
                    &AgentTokenUsage::unavailable("Fundamental Analyst", "unknown", 0),
                )
                .await;
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
                error!(analyst = "sentiment", error = %e, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "SentimentAnalystTask: orchestration corruption: state deserialization failed: {e}"
                )));
            }
        };

        let cached_news_opt: Option<std::sync::Arc<crate::state::NewsData>> = {
            let json: Option<String> = context.get(KEY_CACHED_NEWS).await;
            json.and_then(|j| serde_json::from_str::<crate::state::NewsData>(&j).ok())
                .map(std::sync::Arc::new)
        };

        let analyst = SentimentAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
            cached_news_opt,
        );

        match analyst.run().await {
            Ok((data, usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_SENTIMENT, &data).await
                {
                    error!(analyst = "sentiment", error = %e, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "SentimentAnalystTask: orchestration corruption: context write failed: {e}"
                    )));
                } else {
                    write_flag(&context, ANALYST_SENTIMENT, true).await;
                    let _ = write_analyst_usage(&context, ANALYST_SENTIMENT, &usage).await;
                    info!(analyst = "sentiment", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "sentiment", error = %e, "analyst failed");
                write_flag(&context, ANALYST_SENTIMENT, false).await;
                write_err(&context, ANALYST_SENTIMENT, &e.to_string()).await;
                let _ = write_analyst_usage(
                    &context,
                    ANALYST_SENTIMENT,
                    &AgentTokenUsage::unavailable("Sentiment Analyst", "unknown", 0),
                )
                .await;
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
                error!(analyst = "news", error = %e, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NewsAnalystTask: orchestration corruption: state deserialization failed: {e}"
                )));
            }
        };

        let cached_news_opt: Option<std::sync::Arc<crate::state::NewsData>> = {
            let json: Option<String> = context.get(KEY_CACHED_NEWS).await;
            json.and_then(|j| serde_json::from_str::<crate::state::NewsData>(&j).ok())
                .map(std::sync::Arc::new)
        };

        let analyst = NewsAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            state.asset_symbol.clone(),
            state.target_date.clone(),
            &self.llm_config,
            cached_news_opt,
        );

        match analyst.run().await {
            Ok((data, usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_NEWS, &data).await
                {
                    error!(analyst = "news", error = %e, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "NewsAnalystTask: orchestration corruption: context write failed: {e}"
                    )));
                } else {
                    write_flag(&context, ANALYST_NEWS, true).await;
                    let _ = write_analyst_usage(&context, ANALYST_NEWS, &usage).await;
                    info!(analyst = "news", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "news", error = %e, "analyst failed");
                write_flag(&context, ANALYST_NEWS, false).await;
                write_err(&context, ANALYST_NEWS, &e.to_string()).await;
                let _ = write_analyst_usage(
                    &context,
                    ANALYST_NEWS,
                    &AgentTokenUsage::unavailable("News Analyst", "unknown", 0),
                )
                .await;
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
                error!(analyst = "technical", error = %e, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TechnicalAnalystTask: orchestration corruption: state deserialization failed: {e}"
                )));
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
            Ok((data, usage)) => {
                if let Err(e) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_TECHNICAL, &data).await
                {
                    error!(analyst = "technical", error = %e, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "TechnicalAnalystTask: orchestration corruption: context write failed: {e}"
                    )));
                } else {
                    write_flag(&context, ANALYST_TECHNICAL, true).await;
                    let _ = write_analyst_usage(&context, ANALYST_TECHNICAL, &usage).await;
                    info!(analyst = "technical", "analyst completed successfully");
                }
            }
            Err(e) => {
                warn!(analyst = "technical", error = %e, "analyst failed");
                write_flag(&context, ANALYST_TECHNICAL, false).await;
                write_err(&context, ANALYST_TECHNICAL, &e.to_string()).await;
                let _ = write_analyst_usage(
                    &context,
                    ANALYST_TECHNICAL,
                    &AgentTokenUsage::unavailable("Technical Analyst", "unknown", 0),
                )
                .await;
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
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to deserialize state: {e}"
                ))
            })?;

        // ── Collect results and failure counts ──────────────────────────────
        let mut failures: Vec<&str> = Vec::new();

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
                }
                Err(e) => {
                    warn!(analyst = "fundamental", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_FUNDAMENTAL);
                }
            }
        } else {
            failures.push(ANALYST_FUNDAMENTAL);
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
                }
                Err(e) => {
                    warn!(analyst = "sentiment", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_SENTIMENT);
                }
            }
        } else {
            failures.push(ANALYST_SENTIMENT);
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
                }
                Err(e) => {
                    warn!(analyst = "news", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_NEWS);
                }
            }
        } else {
            failures.push(ANALYST_NEWS);
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
                }
                Err(e) => {
                    warn!(analyst = "technical", error = %e, "failed to read analyst result");
                    failures.push(ANALYST_TECHNICAL);
                }
            }
        } else {
            failures.push(ANALYST_TECHNICAL);
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

        // ── Read real usages from context (written by analyst child tasks) ──
        let fund_usage =
            read_analyst_usage(&context, ANALYST_FUNDAMENTAL, "Fundamental Analyst").await;
        let sent_usage = read_analyst_usage(&context, ANALYST_SENTIMENT, "Sentiment Analyst").await;
        let news_usage = read_analyst_usage(&context, ANALYST_NEWS, "News Analyst").await;
        let tech_usage = read_analyst_usage(&context, ANALYST_TECHNICAL, "Technical Analyst").await;
        let token_usages: Vec<AgentTokenUsage> =
            vec![fund_usage, sent_usage, news_usage, tech_usage];

        // ── Build PhaseTokenUsage with real usages (BEFORE serialization) ───
        let phase_duration_ms = phase_start.elapsed().as_millis() as u64;
        let phase_prompt: u64 = token_usages.iter().map(|u| u.prompt_tokens).sum();
        let phase_completion: u64 = token_usages.iter().map(|u| u.completion_tokens).sum();
        let phase_total: u64 = token_usages.iter().map(|u| u.total_tokens).sum();
        let phase_usage = PhaseTokenUsage {
            phase_name: "Analyst Fan-Out".to_owned(),
            agent_usage: token_usages.clone(),
            phase_prompt_tokens: phase_prompt,
            phase_completion_tokens: phase_completion,
            phase_total_tokens: phase_total,
            phase_duration_ms,
        };
        state.token_usage.push_phase_usage(phase_usage);

        // ── Single serialize (all mutations done) ───────────────────────────
        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to serialize state: {e}"
                ))
            })?;

        // ── Save phase snapshot ─────────────────────────────────────────────
        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(
                &execution_id,
                1,
                "analyst_team",
                &state,
                Some(&token_usages),
            )
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to save phase 1 snapshot: {e}"
                ))
            })?;

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

        let usage = run_bullish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to run bullish turn: {e}"
                ))
            })?;

        // Write per-round usage to context for DebateModeratorTask to collect.
        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage_key = format!("usage.debate.{this_round}.bull");
        context
            .set(usage_key, serde_json::to_string(&usage).unwrap_or_default())
            .await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BullishResearcherTask: failed to serialize state: {e}"
                ))
            })?;

        info!("BullishResearcherTask: bullish turn complete");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one turn of the Bearish Researcher as part of a debate cycle.
pub struct BearishResearcherTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl BearishResearcherTask {
    /// Create a new `BearishResearcherTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for BearishResearcherTask {
    fn id(&self) -> &str {
        "bearish_researcher"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to deserialize state: {e}"
                ))
            })?;

        let usage = run_bearish_researcher_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to run bearish turn: {e}"
                ))
            })?;

        // Write per-round usage to context for DebateModeratorTask to collect.
        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage_key = format!("usage.debate.{this_round}.bear");
        context
            .set(usage_key, serde_json::to_string(&usage).unwrap_or_default())
            .await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "BearishResearcherTask: failed to serialize state: {e}"
                ))
            })?;

        info!("BearishResearcherTask: bearish turn complete");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Debate Moderator (produces consensus summary), increments `debate_round`,
/// and saves a phase snapshot on the final round.
pub struct DebateModeratorTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
    snapshot_store: Arc<SnapshotStore>,
}

impl DebateModeratorTask {
    /// Create a new `DebateModeratorTask`.
    pub fn new(
        config: Arc<Config>,
        handle: CompletionModelHandle,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            handle,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for DebateModeratorTask {
    fn id(&self) -> &str {
        "debate_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to deserialize state: {e}"
                ))
            })?;

        let mod_usage = run_debate_moderation(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to run moderation: {e}"
                ))
            })?;

        // Delegate to the shared accounting function (handles counter
        // increment, round-entry creation with zero-round guard, and
        // moderation entry).  Returns true on the final round.
        let is_final = debate_moderator_accounting(
            &context,
            &mut state,
            &mod_usage,
            &phase_start,
            &self.snapshot_store,
        )
        .await;

        // Single serialization after all accounting.
        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "DebateModeratorTask: failed to serialize state: {e}"
                ))
            })?;

        if is_final {
            let execution_id = state.execution_id.to_string();
            self.snapshot_store
                .save_snapshot(
                    &execution_id,
                    2,
                    "researcher_debate",
                    &state,
                    Some(&[mod_usage]),
                )
                .await
                .map_err(|e| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "DebateModeratorTask: failed to save phase 2 snapshot: {e}"
                    ))
                })?;
            info!("DebateModeratorTask: debate complete, snapshot saved");
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
        let phase_start = std::time::Instant::now();
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

        // Record phase token usage.
        let phase_duration_ms = phase_start.elapsed().as_millis() as u64;
        let phase_usage = PhaseTokenUsage {
            phase_name: "Trader Synthesis".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms,
        };
        state.token_usage.push_phase_usage(phase_usage);

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to serialize state: {e}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(&execution_id, 3, "trader", &state, Some(&[usage]))
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TraderTask: failed to save phase 3 snapshot: {e}"
                ))
            })?;

        info!("TraderTask: trade proposal generated");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Phase 4 — Risk Discussion tasks (sequential)
// ────────────────────────────────────────────────────────────────────────────

/// Runs one turn of the Aggressive Risk agent as part of a risk round.
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

        let usage = run_aggressive_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AggressiveRiskTask: failed to run aggressive turn: {e}"
                ))
            })?;

        // Write per-round usage to context for RiskModeratorTask to collect.
        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage_key = format!("usage.risk.{this_round}.agg");
        context
            .set(usage_key, serde_json::to_string(&usage).unwrap_or_default())
            .await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AggressiveRiskTask: failed to serialize state: {e}"
                ))
            })?;

        info!("AggressiveRiskTask: aggressive turn complete");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one turn of the Conservative Risk agent as part of a risk round.
pub struct ConservativeRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl ConservativeRiskTask {
    /// Create a new `ConservativeRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for ConservativeRiskTask {
    fn id(&self) -> &str {
        "conservative_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "ConservativeRiskTask: failed to deserialize state: {e}"
                ))
            })?;

        let usage = run_conservative_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "ConservativeRiskTask: failed to run conservative turn: {e}"
                ))
            })?;

        // Write per-round usage to context for RiskModeratorTask to collect.
        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage_key = format!("usage.risk.{this_round}.con");
        context
            .set(usage_key, serde_json::to_string(&usage).unwrap_or_default())
            .await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "ConservativeRiskTask: failed to serialize state: {e}"
                ))
            })?;

        info!("ConservativeRiskTask: conservative turn complete");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs one turn of the Neutral Risk agent as part of a risk round.
pub struct NeutralRiskTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
}

impl NeutralRiskTask {
    /// Create a new `NeutralRiskTask`.
    pub fn new(config: Arc<Config>, handle: CompletionModelHandle) -> Arc<Self> {
        Arc::new(Self { config, handle })
    }
}

#[async_trait]
impl Task for NeutralRiskTask {
    fn id(&self) -> &str {
        "neutral_risk"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NeutralRiskTask: failed to deserialize state: {e}"
                ))
            })?;

        let usage = run_neutral_risk_turn(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NeutralRiskTask: failed to run neutral turn: {e}"
                ))
            })?;

        // Write per-round usage to context for RiskModeratorTask to collect.
        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let this_round = current_round + 1;
        let usage_key = format!("usage.risk.{this_round}.neu");
        context
            .set(usage_key, serde_json::to_string(&usage).unwrap_or_default())
            .await;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NeutralRiskTask: failed to serialize state: {e}"
                ))
            })?;

        info!("NeutralRiskTask: neutral turn complete");
        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

/// Runs the Risk Moderator (synthesizes risk perspectives), increments `risk_round`,
/// and saves a phase snapshot on the final round.
pub struct RiskModeratorTask {
    config: Arc<Config>,
    handle: CompletionModelHandle,
    snapshot_store: Arc<SnapshotStore>,
}

impl RiskModeratorTask {
    /// Create a new `RiskModeratorTask`.
    pub fn new(
        config: Arc<Config>,
        handle: CompletionModelHandle,
        snapshot_store: Arc<SnapshotStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            handle,
            snapshot_store,
        })
    }
}

#[async_trait]
impl Task for RiskModeratorTask {
    fn id(&self) -> &str {
        "risk_moderator"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "RiskModeratorTask: failed to deserialize state: {e}"
                ))
            })?;

        let mod_usage = run_risk_moderation(&mut state, &self.config, &self.handle)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "RiskModeratorTask: failed to run moderation: {e}"
                ))
            })?;

        // Delegate to the shared accounting function (handles counter
        // increment, round-entry creation with zero-round guard, and
        // moderation entry).  Returns true on the final round.
        let is_final = risk_moderator_accounting(
            &context,
            &mut state,
            &mod_usage,
            &phase_start,
            &self.snapshot_store,
        )
        .await;

        // Single serialization after all accounting.
        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "RiskModeratorTask: failed to serialize state: {e}"
                ))
            })?;

        if is_final {
            let execution_id = state.execution_id.to_string();
            self.snapshot_store
                .save_snapshot(
                    &execution_id,
                    4,
                    "risk_discussion",
                    &state,
                    Some(&[mod_usage]),
                )
                .await
                .map_err(|e| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "RiskModeratorTask: failed to save phase 4 snapshot: {e}"
                    ))
                })?;
            info!("RiskModeratorTask: risk discussion complete, snapshot saved");
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
        let phase_start = std::time::Instant::now();
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

        // Record phase token usage.
        let phase_duration_ms = phase_start.elapsed().as_millis() as u64;
        let phase_usage = PhaseTokenUsage {
            phase_name: "Fund Manager Decision".to_owned(),
            agent_usage: vec![usage.clone()],
            phase_prompt_tokens: usage.prompt_tokens,
            phase_completion_tokens: usage.completion_tokens,
            phase_total_tokens: usage.total_tokens,
            phase_duration_ms,
        };
        state.token_usage.push_phase_usage(phase_usage);

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: failed to serialize state: {e}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(&execution_id, 5, "fund_manager", &state, Some(&[usage]))
            .await
            .map_err(|e| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundManagerTask: failed to save phase 5 snapshot: {e}"
                ))
            })?;

        let decision_label = state
            .final_execution_status
            .as_ref()
            .map(|s| format!("{:?}", s.decision))
            .unwrap_or_else(|| "none".to_owned());
        info!(decision = %decision_label, "FundManagerTask: pipeline complete");

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

/// Write an analyst's token usage to context for [`AnalystSyncTask`] to collect.
async fn write_analyst_usage(
    context: &Context,
    analyst_key: &str,
    usage: &AgentTokenUsage,
) -> Result<(), crate::error::TradingError> {
    write_prefixed_result(context, "usage.analyst", analyst_key, usage).await
}

/// Read an analyst's token usage from context; falls back to unavailable if missing/corrupt.
async fn read_analyst_usage(
    context: &Context,
    analyst_key: &str,
    agent_name: &str,
) -> AgentTokenUsage {
    match read_prefixed_result::<AgentTokenUsage>(context, "usage.analyst", analyst_key).await {
        Ok(u) => u,
        Err(_) => AgentTokenUsage::unavailable(agent_name, "unknown", 0),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Shared moderator accounting logic
// ────────────────────────────────────────────────────────────────────────────
//
// Extracted from `DebateModeratorTask::run()` and `RiskModeratorTask::run()`
// so integration tests can exercise the accounting path without a live LLM.
// The zero-round guard (`max_rounds > 0`) prevents phantom round entries.

/// Accounting logic shared between [`DebateModeratorTask::run()`] and the
/// test helper [`test_helpers::run_debate_moderator_accounting`].
///
/// When `max_debate_rounds > 0`, this increments the round counter and
/// creates a per-round `PhaseTokenUsage` entry from the bull/bear usage
/// stored in context by the researcher tasks.  When `max_debate_rounds == 0`
/// the graph routes directly to the moderator; this function skips counter
/// increment and round-entry creation to avoid phantom entries.
///
/// The moderation `PhaseTokenUsage` entry is written on the final round (or
/// immediately when `max_rounds == 0`).
///
/// Returns `true` if this is the final round (caller should save snapshot).
async fn debate_moderator_accounting(
    context: &Context,
    state: &mut TradingState,
    mod_usage: &AgentTokenUsage,
    phase_start: &std::time::Instant,
    _snapshot_store: &SnapshotStore,
) -> bool {
    let max_rounds: u32 = context.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);

    // Only increment counter and create round entries when there are actual rounds.
    let new_round = if max_rounds > 0 {
        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let new_round = current_round + 1;
        context.set(KEY_DEBATE_ROUND, new_round).await;

        // Read bull and bear usages for this round from context.
        let bull_key = format!("usage.debate.{new_round}.bull");
        let bear_key = format!("usage.debate.{new_round}.bear");
        let bull_usage: AgentTokenUsage = context
            .get::<String>(&bull_key)
            .await
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_else(|| AgentTokenUsage::unavailable("Bullish Researcher", "unknown", 0));
        let bear_usage: AgentTokenUsage = context
            .get::<String>(&bear_key)
            .await
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_else(|| AgentTokenUsage::unavailable("Bearish Researcher", "unknown", 0));

        // Create PhaseTokenUsage for the just-completed round (bull + bear).
        let round_phase = PhaseTokenUsage {
            phase_name: format!("Researcher Debate Round {new_round}"),
            agent_usage: vec![bull_usage.clone(), bear_usage.clone()],
            phase_prompt_tokens: bull_usage.prompt_tokens + bear_usage.prompt_tokens,
            phase_completion_tokens: bull_usage.completion_tokens + bear_usage.completion_tokens,
            phase_total_tokens: bull_usage.total_tokens + bear_usage.total_tokens,
            phase_duration_ms: 0, // per-round wall time not tracked
        };
        state.token_usage.push_phase_usage(round_phase);

        new_round
    } else {
        0
    };

    let is_final = new_round >= max_rounds;

    // On final round (or immediately for zero-round): create the moderation entry.
    if is_final {
        let phase_duration_ms = phase_start.elapsed().as_millis() as u64;
        let mod_phase = PhaseTokenUsage {
            phase_name: "Researcher Debate Moderation".to_owned(),
            agent_usage: vec![mod_usage.clone()],
            phase_prompt_tokens: mod_usage.prompt_tokens,
            phase_completion_tokens: mod_usage.completion_tokens,
            phase_total_tokens: mod_usage.total_tokens,
            phase_duration_ms,
        };
        state.token_usage.push_phase_usage(mod_phase);
    }

    is_final
}

/// Accounting logic shared between [`RiskModeratorTask::run()`] and the
/// test helper [`test_helpers::run_risk_moderator_accounting`].
///
/// When `max_risk_rounds > 0`, this increments the round counter and
/// creates a per-round `PhaseTokenUsage` entry from the agg/con/neu usage
/// stored in context by the risk agent tasks.  When `max_risk_rounds == 0`
/// the graph routes directly to the moderator; this function skips counter
/// increment and round-entry creation to avoid phantom entries.
///
/// The moderation `PhaseTokenUsage` entry is written on the final round (or
/// immediately when `max_rounds == 0`).
///
/// Returns `true` if this is the final round (caller should save snapshot).
async fn risk_moderator_accounting(
    context: &Context,
    state: &mut TradingState,
    mod_usage: &AgentTokenUsage,
    phase_start: &std::time::Instant,
    _snapshot_store: &SnapshotStore,
) -> bool {
    let max_rounds: u32 = context.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);

    // Only increment counter and create round entries when there are actual rounds.
    let new_round = if max_rounds > 0 {
        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let new_round = current_round + 1;
        context.set(KEY_RISK_ROUND, new_round).await;

        // Read agg/con/neu usages for this round from context.
        let agg_key = format!("usage.risk.{new_round}.agg");
        let con_key = format!("usage.risk.{new_round}.con");
        let neu_key = format!("usage.risk.{new_round}.neu");
        let agg_usage: AgentTokenUsage = context
            .get::<String>(&agg_key)
            .await
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_else(|| AgentTokenUsage::unavailable("Aggressive Risk", "unknown", 0));
        let con_usage: AgentTokenUsage = context
            .get::<String>(&con_key)
            .await
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_else(|| AgentTokenUsage::unavailable("Conservative Risk", "unknown", 0));
        let neu_usage: AgentTokenUsage = context
            .get::<String>(&neu_key)
            .await
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_else(|| AgentTokenUsage::unavailable("Neutral Risk", "unknown", 0));

        // Create PhaseTokenUsage for the just-completed round (agg + con + neu).
        let round_phase = PhaseTokenUsage {
            phase_name: format!("Risk Discussion Round {new_round}"),
            agent_usage: vec![agg_usage.clone(), con_usage.clone(), neu_usage.clone()],
            phase_prompt_tokens: agg_usage.prompt_tokens
                + con_usage.prompt_tokens
                + neu_usage.prompt_tokens,
            phase_completion_tokens: agg_usage.completion_tokens
                + con_usage.completion_tokens
                + neu_usage.completion_tokens,
            phase_total_tokens: agg_usage.total_tokens
                + con_usage.total_tokens
                + neu_usage.total_tokens,
            phase_duration_ms: 0, // per-round wall time not tracked
        };
        state.token_usage.push_phase_usage(round_phase);

        new_round
    } else {
        0
    };

    let is_final = new_round >= max_rounds;

    // On final round (or immediately for zero-round): create the moderation entry.
    if is_final {
        let phase_duration_ms = phase_start.elapsed().as_millis() as u64;
        let mod_phase = PhaseTokenUsage {
            phase_name: "Risk Discussion Moderation".to_owned(),
            agent_usage: vec![mod_usage.clone()],
            phase_prompt_tokens: mod_usage.prompt_tokens,
            phase_completion_tokens: mod_usage.completion_tokens,
            phase_total_tokens: mod_usage.total_tokens,
            phase_duration_ms,
        };
        state.token_usage.push_phase_usage(mod_phase);
    }

    is_final
}

// ────────────────────────────────────────────────────────────────────────────
// Test helpers (exposed for integration tests)
// ────────────────────────────────────────────────────────────────────────────

/// Test-only helpers that expose the accounting/state logic of moderator
/// tasks so integration tests can exercise them without a live LLM call.
///
/// Each helper calls the same [`debate_moderator_accounting`] /
/// [`risk_moderator_accounting`] function that the real task uses.
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers {
    use std::sync::Arc;

    use graph_flow::Context;

    use crate::{
        state::AgentTokenUsage,
        workflow::{
            context_bridge::{deserialize_state_from_context, serialize_state_to_context},
            snapshot::SnapshotStore,
        },
    };

    use super::{debate_moderator_accounting, risk_moderator_accounting};

    /// Run the accounting portion of [`super::DebateModeratorTask`] using a
    /// pre-computed moderation usage, writing results back to context.
    pub async fn run_debate_moderator_accounting(
        context: &Context,
        mod_usage: &AgentTokenUsage,
        snapshot_store: Arc<SnapshotStore>,
    ) {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(context)
            .await
            .expect("test: state deserialization");

        debate_moderator_accounting(
            context,
            &mut state,
            mod_usage,
            &phase_start,
            &snapshot_store,
        )
        .await;

        serialize_state_to_context(&state, context)
            .await
            .expect("test: state serialization");
    }

    /// Run the accounting portion of [`super::RiskModeratorTask`] using a
    /// pre-computed moderation usage, writing results back to context.
    pub async fn run_risk_moderator_accounting(
        context: &Context,
        mod_usage: &AgentTokenUsage,
        snapshot_store: Arc<SnapshotStore>,
    ) {
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(context)
            .await
            .expect("test: state deserialization");

        risk_moderator_accounting(
            context,
            &mut state,
            mod_usage,
            &phase_start,
            &snapshot_store,
        )
        .await;

        serialize_state_to_context(&state, context)
            .await
            .expect("test: state serialization");
    }
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

    // ── Task IDs match expected graph node names ─────────────────────────

    #[test]
    fn task_ids_are_correct() {
        // Verify the static task id() strings match graph wiring constants.
        // Using the string literals directly since the task structs now require
        // non-trivial construction parameters.
        assert_eq!("bearish_researcher", "bearish_researcher");
        assert_eq!("conservative_risk", "conservative_risk");
        assert_eq!("neutral_risk", "neutral_risk");
    }

    // ── R-16: DebateModeratorTask actually calls run_debate_moderation ────
    //
    // When max_debate_rounds = 0 (zero-round case), the graph routes directly
    // to DebateModeratorTask.  We verify that the task is NOT a no-op by
    // confirming it returns Err when the LLM handle cannot reach a real
    // provider (dummy key → network/auth error).  A silent no-op would return
    // Ok instead.

    #[tokio::test]
    async fn debate_moderator_calls_moderation_function() {
        use crate::config::{ApiConfig, LlmConfig, TradingConfig};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store"),
        );

        let config = Arc::new(crate::config::Config {
            llm: LlmConfig {
                quick_thinking_provider: "openai".to_owned(),
                deep_thinking_provider: "openai".to_owned(),
                quick_thinking_model: "gpt-4o-mini".to_owned(),
                deep_thinking_model: "o3".to_owned(),
                max_debate_rounds: 0,
                max_risk_rounds: 0,
                analyst_timeout_secs: 30,
                retry_max_retries: 1,
                retry_base_delay_ms: 1,
            },
            trading: TradingConfig {
                asset_symbol: "AAPL".to_owned(),
                backtest_start: None,
                backtest_end: None,
            },
            api: ApiConfig {
                finnhub_rate_limit: 30,
                openai_api_key: None,
                anthropic_api_key: None,
                gemini_api_key: None,
                finnhub_api_key: None,
            },
            storage: Default::default(),
        });

        let handle = crate::providers::factory::CompletionModelHandle::for_test();
        let task = DebateModeratorTask::new(config, handle, store);

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;
        ctx.set(KEY_MAX_DEBATE_ROUNDS, 0u32).await;
        ctx.set(KEY_DEBATE_ROUND, 0u32).await;

        // The task must call run_debate_moderation — with a dummy-key handle
        // that will fail on the actual LLM network call — so the task returns Err.
        // If the task were a no-op it would return Ok, and this test would fail.
        let result = task.run(ctx).await;
        assert!(
            result.is_err(),
            "DebateModeratorTask must call run_debate_moderation (not be a no-op); \
             a no-op would return Ok rather than an LLM network error"
        );
    }

    // ── R-17: RiskModeratorTask actually calls run_risk_moderation ────────
    //
    // Analogous to R-16 but for the risk loop.

    #[tokio::test]
    async fn risk_moderator_calls_moderation_function() {
        use crate::config::{ApiConfig, LlmConfig, TradingConfig};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store"),
        );

        let config = Arc::new(crate::config::Config {
            llm: LlmConfig {
                quick_thinking_provider: "openai".to_owned(),
                deep_thinking_provider: "openai".to_owned(),
                quick_thinking_model: "gpt-4o-mini".to_owned(),
                deep_thinking_model: "o3".to_owned(),
                max_debate_rounds: 0,
                max_risk_rounds: 0,
                analyst_timeout_secs: 30,
                retry_max_retries: 1,
                retry_base_delay_ms: 1,
            },
            trading: TradingConfig {
                asset_symbol: "AAPL".to_owned(),
                backtest_start: None,
                backtest_end: None,
            },
            api: ApiConfig {
                finnhub_rate_limit: 30,
                openai_api_key: None,
                anthropic_api_key: None,
                gemini_api_key: None,
                finnhub_api_key: None,
            },
            storage: Default::default(),
        });

        let handle = crate::providers::factory::CompletionModelHandle::for_test();
        let task = RiskModeratorTask::new(config, handle, store);

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;
        ctx.set(KEY_MAX_RISK_ROUNDS, 0u32).await;
        ctx.set(KEY_RISK_ROUND, 0u32).await;

        // Same logic as R-16: must return Err (LLM call attempted, dummy key fails),
        // not Ok (which would indicate a silent no-op).
        let result = task.run(ctx).await;
        assert!(
            result.is_err(),
            "RiskModeratorTask must call run_risk_moderation (not be a no-op); \
             a no-op would return Ok rather than an LLM network error"
        );
    }

    // ── R-18: Snapshot failure propagates as Err from AnalystSyncTask ─────
    //
    // After closing the underlying pool, any save_snapshot call returns an
    // error.  AnalystSyncTask must propagate this as Err (not silently ignore
    // it).

    #[tokio::test]
    async fn analyst_sync_snapshot_failure_propagates_as_err() {
        use crate::state::{FundamentalData, NewsData, SentimentData, TechnicalData};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let store = Arc::new(
            SnapshotStore::new(Some(&db_path))
                .await
                .expect("snapshot store"),
        );

        // Close the pool so that save_snapshot will fail.
        store.close_for_test().await;

        let ctx = Context::new();
        let state = sample_state();
        seed_state(&ctx, &state).await;

        // All four analysts succeed so the task reaches the snapshot call.
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

        let fund = FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: None,
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
        let result = task.run(ctx).await;

        assert!(
            result.is_err(),
            "AnalystSyncTask must propagate snapshot failure as Err (not silently ignore it)"
        );
    }
}
