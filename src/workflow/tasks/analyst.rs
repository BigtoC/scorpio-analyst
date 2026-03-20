use std::sync::Arc;

use async_trait::async_trait;
use graph_flow::{Context, NextAction, Task, TaskResult};
use serde::de::DeserializeOwned;
use tracing::{error, info, warn};

use crate::{
    agents::analyst::{FundamentalAnalyst, NewsAnalyst, SentimentAnalyst, TechnicalAnalyst},
    config::LlmConfig,
    data::{FinnhubClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    state::{
        AgentTokenUsage, FundamentalData, NewsData, PhaseTokenUsage, SentimentData, TechnicalData,
        TradingState,
    },
    workflow::{
        context_bridge::{
            deserialize_state_from_context, read_prefixed_result, serialize_state_to_context,
            write_prefixed_result,
        },
        snapshot::{SnapshotPhase, SnapshotStore},
        tasks::common::{
            ANALYST_FUNDAMENTAL, ANALYST_NEWS, ANALYST_PREFIX, ANALYST_SENTIMENT,
            ANALYST_TECHNICAL, OK_SUFFIX, read_analyst_usage, write_analyst_usage, write_err,
            write_flag,
        },
    },
};

/// Runs the phase-1 fundamental analyst child task.
///
/// On success the task writes typed analyst output and token usage into context,
/// marks the analyst as successful, and returns [`NextAction::Continue`] so the
/// fan-out can finish. Orchestration corruption returns `Err` to fail the fan-out
/// closed, while analyst runtime failures degrade gracefully via context flags.
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
            Ok(state) => state,
            Err(error) => {
                error!(analyst = "fundamental", error = %error, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "FundamentalAnalystTask: orchestration corruption: state deserialization failed: {error}"
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
                if let Err(error) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_FUNDAMENTAL, &data)
                        .await
                {
                    error!(analyst = "fundamental", error = %error, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "FundamentalAnalystTask: orchestration corruption: context write failed: {error}"
                    )));
                }

                write_flag(&context, ANALYST_FUNDAMENTAL, true).await;
                let _ = write_analyst_usage(&context, ANALYST_FUNDAMENTAL, &usage).await;
                info!(analyst = "fundamental", "analyst completed successfully");
            }
            Err(error) => {
                warn!(analyst = "fundamental", error = %error, "analyst failed");
                write_flag(&context, ANALYST_FUNDAMENTAL, false).await;
                write_err(&context, ANALYST_FUNDAMENTAL, &error.to_string()).await;
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

/// Runs the phase-1 sentiment analyst child task.
///
/// On success the task writes typed analyst output and token usage into context,
/// marks the analyst as successful, and returns [`NextAction::Continue`] so the
/// fan-out can finish. Orchestration corruption returns `Err` to fail the fan-out
/// closed, while analyst runtime failures degrade gracefully via context flags.
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
            Ok(state) => state,
            Err(error) => {
                error!(analyst = "sentiment", error = %error, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "SentimentAnalystTask: orchestration corruption: state deserialization failed: {error}"
                )));
            }
        };

        let cached_news_opt = {
            let json: Option<String> = context.get(super::KEY_CACHED_NEWS).await;
            json.and_then(|value| serde_json::from_str::<crate::state::NewsData>(&value).ok())
                .map(Arc::new)
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
                if let Err(error) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_SENTIMENT, &data).await
                {
                    error!(analyst = "sentiment", error = %error, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "SentimentAnalystTask: orchestration corruption: context write failed: {error}"
                    )));
                }

                write_flag(&context, ANALYST_SENTIMENT, true).await;
                let _ = write_analyst_usage(&context, ANALYST_SENTIMENT, &usage).await;
                info!(analyst = "sentiment", "analyst completed successfully");
            }
            Err(error) => {
                warn!(analyst = "sentiment", error = %error, "analyst failed");
                write_flag(&context, ANALYST_SENTIMENT, false).await;
                write_err(&context, ANALYST_SENTIMENT, &error.to_string()).await;
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

/// Runs the phase-1 news analyst child task.
///
/// On success the task writes typed analyst output and token usage into context,
/// marks the analyst as successful, and returns [`NextAction::Continue`] so the
/// fan-out can finish. Orchestration corruption returns `Err` to fail the fan-out
/// closed, while analyst runtime failures degrade gracefully via context flags.
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
            Ok(state) => state,
            Err(error) => {
                error!(analyst = "news", error = %error, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NewsAnalystTask: orchestration corruption: state deserialization failed: {error}"
                )));
            }
        };

        let cached_news_opt = {
            let json: Option<String> = context.get(super::KEY_CACHED_NEWS).await;
            json.and_then(|value| serde_json::from_str::<crate::state::NewsData>(&value).ok())
                .map(Arc::new)
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
                if let Err(error) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_NEWS, &data).await
                {
                    error!(analyst = "news", error = %error, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "NewsAnalystTask: orchestration corruption: context write failed: {error}"
                    )));
                }

                write_flag(&context, ANALYST_NEWS, true).await;
                let _ = write_analyst_usage(&context, ANALYST_NEWS, &usage).await;
                info!(analyst = "news", "analyst completed successfully");
            }
            Err(error) => {
                warn!(analyst = "news", error = %error, "analyst failed");
                write_flag(&context, ANALYST_NEWS, false).await;
                write_err(&context, ANALYST_NEWS, &error.to_string()).await;
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

/// Runs the phase-1 technical analyst child task.
///
/// On success the task writes typed analyst output and token usage into context,
/// marks the analyst as successful, and returns [`NextAction::Continue`] so the
/// fan-out can finish. Orchestration corruption returns `Err` to fail the fan-out
/// closed, while analyst runtime failures degrade gracefully via context flags.
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
            Ok(state) => state,
            Err(error) => {
                error!(analyst = "technical", error = %error, "orchestration corruption: failed to deserialize state");
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "TechnicalAnalystTask: orchestration corruption: state deserialization failed: {error}"
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
                if let Err(error) =
                    write_prefixed_result(&context, ANALYST_PREFIX, ANALYST_TECHNICAL, &data).await
                {
                    error!(analyst = "technical", error = %error, "orchestration corruption: failed to write result to context");
                    return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                        "TechnicalAnalystTask: orchestration corruption: context write failed: {error}"
                    )));
                }

                write_flag(&context, ANALYST_TECHNICAL, true).await;
                let _ = write_analyst_usage(&context, ANALYST_TECHNICAL, &usage).await;
                info!(analyst = "technical", "analyst completed successfully");
            }
            Err(error) => {
                warn!(analyst = "technical", error = %error, "analyst failed");
                write_flag(&context, ANALYST_TECHNICAL, false).await;
                write_err(&context, ANALYST_TECHNICAL, &error.to_string()).await;
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

/// Aggregates phase-1 analyst fan-out results.
///
/// The task reads typed analyst results and usage from context, merges all
/// successful analyst outputs into [`TradingState`], applies the degradation
/// policy (`0-1` failures continue, `2+` abort), persists the phase-1 snapshot,
/// and returns [`NextAction::Continue`] or [`NextAction::End`] accordingly.
pub struct AnalystSyncTask {
    snapshot_store: Arc<SnapshotStore>,
}

impl AnalystSyncTask {
    /// Create a new `AnalystSyncTask`.
    pub fn new(snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Arc::new(Self { snapshot_store })
    }
}

async fn merge_analyst_result<T, F>(
    context: &Context,
    state: &mut TradingState,
    failures: &mut Vec<&'static str>,
    analyst_key: &'static str,
    on_success: F,
) where
    T: DeserializeOwned,
    F: FnOnce(&mut TradingState, T),
{
    let ok_key = format!("{ANALYST_PREFIX}.{analyst_key}.{OK_SUFFIX}");
    let succeeded: bool = context.get(&ok_key).await.unwrap_or(false);

    if !succeeded {
        failures.push(analyst_key);
        return;
    }

    match read_prefixed_result::<T>(context, ANALYST_PREFIX, analyst_key).await {
        Ok(data) => on_success(state, data),
        Err(error) => {
            warn!(analyst = analyst_key, error = %error, "failed to read analyst result");
            failures.push(analyst_key);
        }
    }
}

#[async_trait]
impl Task for AnalystSyncTask {
    fn id(&self) -> &str {
        "analyst_sync"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = "analyst_sync", phase = 1, "task started");
        let phase_start = std::time::Instant::now();
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to deserialize state: {error}"
                ))
            })?;

        let mut failures = Vec::new();

        merge_analyst_result::<FundamentalData, _>(
            &context,
            &mut state,
            &mut failures,
            ANALYST_FUNDAMENTAL,
            |state, data| state.fundamental_metrics = Some(data),
        )
        .await;
        merge_analyst_result::<SentimentData, _>(
            &context,
            &mut state,
            &mut failures,
            ANALYST_SENTIMENT,
            |state, data| state.market_sentiment = Some(data),
        )
        .await;
        merge_analyst_result::<NewsData, _>(
            &context,
            &mut state,
            &mut failures,
            ANALYST_NEWS,
            |state, data| state.macro_news = Some(data),
        )
        .await;
        merge_analyst_result::<TechnicalData, _>(
            &context,
            &mut state,
            &mut failures,
            ANALYST_TECHNICAL,
            |state, data| state.technical_indicators = Some(data),
        )
        .await;

        let failure_count = failures.len();
        if failure_count >= 2 {
            error!(
                failures = ?failures,
                "AnalystSyncTask: {failure_count}/4 analysts failed — aborting pipeline"
            );
            return Ok(TaskResult::new(
                Some(format!(
                    "{failure_count}/4 analysts failed — pipeline aborted"
                )),
                NextAction::End,
            ));
        }

        let token_usages = vec![
            read_analyst_usage(&context, ANALYST_FUNDAMENTAL, "Fundamental Analyst").await,
            read_analyst_usage(&context, ANALYST_SENTIMENT, "Sentiment Analyst").await,
            read_analyst_usage(&context, ANALYST_NEWS, "News Analyst").await,
            read_analyst_usage(&context, ANALYST_TECHNICAL, "Technical Analyst").await,
        ];

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Analyst Fan-Out".to_owned(),
            phase_prompt_tokens: token_usages.iter().map(|usage| usage.prompt_tokens).sum(),
            phase_completion_tokens: token_usages
                .iter()
                .map(|usage| usage.completion_tokens)
                .sum(),
            phase_total_tokens: token_usages.iter().map(|usage| usage.total_tokens).sum(),
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
            agent_usage: token_usages.clone(),
        });

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to serialize state: {error}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(
                &execution_id,
                SnapshotPhase::AnalystTeam,
                &state,
                Some(&token_usages),
            )
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to save phase 1 snapshot: {error}"
                ))
            })?;

        info!(task = "analyst_sync", phase = 1, "snapshot saved");
        info!(
            failures = failure_count,
            "AnalystSyncTask: phase 1 complete"
        );
        info!(phase = 1, phase_name = "analyst_team", "phase complete");

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}
