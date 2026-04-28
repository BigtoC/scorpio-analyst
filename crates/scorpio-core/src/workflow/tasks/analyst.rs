use std::sync::Arc;
use std::time::Duration;
use std::{fmt::Debug, future::Future};

use async_trait::async_trait;
use chrono::Utc;
use graph_flow::{Context, NextAction, Task, TaskResult};
use serde::de::DeserializeOwned;
use tokio::time::timeout;
use tracing::{error, info, warn};
use yfinance_rs::{
    analysis::EarningsTrendRow,
    fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow, ShareCount},
    profile::Profile,
};

use crate::{
    agents::analyst::{
        AnalystId, FundamentalAnalyst, NewsAnalyst, SentimentAnalyst, TechnicalAnalyst,
    },
    config::LlmConfig,
    data::{FinnhubClient, FredClient, YFinanceClient},
    providers::factory::CompletionModelHandle,
    state::{
        AgentTokenUsage, AssetShape, DataCoverageReport, DerivedValuation, EvidenceKind,
        EvidenceRecord, EvidenceSource, FundamentalData, NewsData, PhaseTokenUsage,
        ProvenanceSummary, ScenarioValuation, SentimentData, TechnicalData, TradingState,
        derive_valuation,
    },
    valuation::ValuatorRegistry,
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

// ─── Stage 1 source-mapping constants ─────────────────────────────────────────

/// Fixed provider for fundamentals in Stage 1.
const PROVIDER_FINNHUB: &str = "finnhub";
/// Fixed provider for macro/news (FRED) in Stage 1.
const PROVIDER_FRED: &str = "fred";
/// Fixed provider for technical data in Stage 1.
const PROVIDER_YFINANCE: &str = "yfinance";

/// Build a single-provider [`EvidenceSource`] with Stage 1 defaults.
fn stage1_source(provider: &str, datasets: Vec<String>) -> EvidenceSource {
    EvidenceSource {
        provider: provider.to_owned(),
        datasets,
        fetched_at: Utc::now(),
        effective_at: None,
        url: None,
        citation: None,
    }
}

async fn read_cached_news(
    task_name: &str,
    context: &Context,
) -> graph_flow::Result<Option<Arc<NewsData>>> {
    let json: Option<String> = context.get(super::KEY_CACHED_NEWS).await;
    json.map(|value| {
        serde_json::from_str::<NewsData>(&value).map(Arc::new).map_err(|error| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "{task_name}: orchestration corruption: cached news deserialization failed: {error}"
            ))
        })
    })
    .transpose()
}

fn required_inputs_for_state(state: &TradingState) -> Vec<String> {
    state
        .analysis_runtime_policy
        .as_ref()
        .map(|policy| policy.required_inputs.clone())
        .unwrap_or_else(|| {
            vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ]
        })
}

/// Resolve the active analyst id set for this cycle from the pack's
/// `required_inputs`. Entries that don't map to a known analyst are dropped.
fn active_analyst_ids(state: &TradingState) -> Vec<AnalystId> {
    required_inputs_for_state(state)
        .iter()
        .filter_map(|s| AnalystId::from_required_input(s))
        .collect()
}

fn input_missing(state: &TradingState, input: &str) -> bool {
    match input {
        "fundamentals" => state.evidence_fundamental().is_none(),
        "sentiment" => state.evidence_sentiment().is_none(),
        "news" => state.evidence_news().is_none(),
        "technical" => state.evidence_technical().is_none(),
        _ => false,
    }
}

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

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "FundamentalAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let analyst = FundamentalAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            &state,
            policy,
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

        let cached_news_opt = read_cached_news("SentimentAnalystTask", &context).await?;

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "SentimentAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let analyst = SentimentAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            &state,
            policy,
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
    fred: FredClient,
    llm_config: LlmConfig,
}

impl NewsAnalystTask {
    /// Create a new `NewsAnalystTask`.
    pub fn new(
        handle: CompletionModelHandle,
        finnhub: FinnhubClient,
        fred: FredClient,
        llm_config: LlmConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            handle,
            finnhub,
            fred,
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

        let cached_news_opt = read_cached_news("NewsAnalystTask", &context).await?;

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "NewsAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let analyst = NewsAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            self.fred.clone(),
            &state,
            policy,
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

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "TechnicalAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let analyst = TechnicalAnalyst::new(
            self.handle.clone(),
            self.yfinance.clone(),
            &state,
            policy,
            &self.llm_config,
        )
        .map_err(|e| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "TechnicalAnalystTask: failed to construct analyst: {e}"
            ))
        })?;

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
    yfinance: YFinanceClient,
    valuation_fetch_timeout: Duration,
}

impl AnalystSyncTask {
    /// Create a new `AnalystSyncTask`.
    #[cfg_attr(not(any(test, feature = "test-helpers")), allow(dead_code))]
    #[must_use]
    pub fn new(snapshot_store: Arc<SnapshotStore>) -> Arc<Self> {
        Self::with_yfinance(
            snapshot_store,
            YFinanceClient::default(),
            Duration::from_secs(30),
        )
    }

    /// Create a new `AnalystSyncTask` with an explicit Yahoo Finance client.
    ///
    /// `yfinance` is used to fetch financial statement data for deterministic
    /// valuation derivation after the analyst fan-out completes. In tests,
    /// [`YFinanceClient::default`] may be supplied; network-unavailable calls
    /// degrade gracefully to `NotAssessed` without aborting the cycle.
    #[must_use]
    pub fn with_yfinance(
        snapshot_store: Arc<SnapshotStore>,
        yfinance: YFinanceClient,
        valuation_fetch_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            snapshot_store,
            yfinance,
            valuation_fetch_timeout,
        })
    }
}

#[derive(Debug)]
struct ValuationInputs {
    profile: Option<Profile>,
    cashflow: Option<Vec<CashflowRow>>,
    balance: Option<Vec<BalanceSheetRow>>,
    income: Option<Vec<IncomeStatementRow>>,
    shares: Option<Vec<ShareCount>>,
    trend: Option<Vec<EarningsTrendRow>>,
}

async fn fetch_valuation_inputs(
    yfinance: &YFinanceClient,
    symbol: &str,
    fetch_timeout: Duration,
) -> ValuationInputs {
    let (profile, cashflow, balance, income, shares, trend) = tokio::join!(
        fetch_with_timeout(
            symbol,
            "profile",
            fetch_timeout,
            yfinance.get_profile(symbol)
        ),
        fetch_with_timeout(
            symbol,
            "quarterly_cashflow",
            fetch_timeout,
            yfinance.get_quarterly_cashflow(symbol),
        ),
        fetch_with_timeout(
            symbol,
            "quarterly_balance_sheet",
            fetch_timeout,
            yfinance.get_quarterly_balance_sheet(symbol),
        ),
        fetch_with_timeout(
            symbol,
            "quarterly_income_stmt",
            fetch_timeout,
            yfinance.get_quarterly_income_stmt(symbol),
        ),
        fetch_with_timeout(
            symbol,
            "quarterly_shares",
            fetch_timeout,
            yfinance.get_quarterly_shares(symbol),
        ),
        fetch_with_timeout(
            symbol,
            "earnings_trend",
            fetch_timeout,
            yfinance.get_earnings_trend(symbol),
        ),
    );

    ValuationInputs {
        profile,
        cashflow,
        balance,
        income,
        shares,
        trend,
    }
}

async fn fetch_with_timeout<T, F>(
    symbol: &str,
    field: &'static str,
    fetch_timeout: Duration,
    fetch: F,
) -> Option<T>
where
    T: Debug,
    F: Future<Output = Option<T>>,
{
    match timeout(fetch_timeout, fetch).await {
        Ok(value) => value,
        Err(_) => {
            warn!(
                symbol,
                field,
                timeout_secs = fetch_timeout.as_secs_f64(),
                "valuation fetch timed out"
            );
            None
        }
    }
}

async fn merge_analyst_result<T, F, G>(
    context: &Context,
    state: &mut TradingState,
    failures: &mut Vec<&'static str>,
    analyst_key: &'static str,
    on_success: F,
    on_evidence: G,
) where
    T: DeserializeOwned + Clone,
    F: FnOnce(&mut TradingState, T),
    G: FnOnce(&mut TradingState, T),
{
    let ok_key = format!("{ANALYST_PREFIX}.{analyst_key}.{OK_SUFFIX}");
    let succeeded: bool = context.get(&ok_key).await.unwrap_or(false);

    if !succeeded {
        failures.push(analyst_key);
        return;
    }

    match read_prefixed_result::<T>(context, ANALYST_PREFIX, analyst_key).await {
        Ok(data) => {
            let data_clone = data.clone();
            on_success(state, data);
            on_evidence(state, data_clone);
        }
        Err(error) => {
            warn!(analyst = analyst_key, error = %error, "failed to read analyst result");
            failures.push(analyst_key);
        }
    }
}

fn no_valuator_selected(asset_shape: AssetShape) -> DerivedValuation {
    DerivedValuation {
        asset_shape,
        scenario: ScenarioValuation::NotAssessed {
            reason: "no_valuator_selected".to_owned(),
        },
    }
}

fn derive_runtime_valuation(
    state: &TradingState,
    valuation_inputs: &ValuationInputs,
    current_price: Option<f64>,
) -> DerivedValuation {
    let provisional = derive_valuation(
        valuation_inputs.profile.clone(),
        valuation_inputs.cashflow.as_deref(),
        valuation_inputs.balance.as_deref(),
        valuation_inputs.income.as_deref(),
        valuation_inputs.shares.as_deref(),
        valuation_inputs.trend.as_deref(),
        current_price,
    );

    let Some(policy) = state.analysis_runtime_policy.as_ref() else {
        return provisional;
    };

    let Some(valuator_id) = policy
        .valuator_selection
        .get(&provisional.asset_shape)
        .copied()
    else {
        return match provisional.asset_shape {
            AssetShape::Fund | AssetShape::Unknown => provisional,
            _ => no_valuator_selected(provisional.asset_shape),
        };
    };

    let registry = ValuatorRegistry::equity_baseline();
    let Some(valuator) = registry.get(valuator_id) else {
        return no_valuator_selected(provisional.asset_shape);
    };

    valuator.assess(
        crate::valuation::ValuationInputs {
            profile: valuation_inputs.profile.clone(),
            cashflow: valuation_inputs.cashflow.as_deref(),
            balance: valuation_inputs.balance.as_deref(),
            income: valuation_inputs.income.as_deref(),
            shares: valuation_inputs.shares.as_deref(),
            earnings_trend: valuation_inputs.trend.as_deref(),
            current_price,
        },
        &provisional.asset_shape,
    )
}

#[async_trait]
impl Task for AnalystSyncTask {
    fn id(&self) -> &str {
        "analyst_sync"
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        info!(task = "analyst_sync", phase = 1, "task started");
        let mut state = deserialize_state_from_context(&context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "AnalystSyncTask: failed to deserialize state: {error}"
                ))
            })?;

        let active_ids = active_analyst_ids(&state);
        let active_total = active_ids.len();
        let mut failures = Vec::new();

        // Only merge for analysts the active pack declared — keeps byte-
        // identical behaviour for the equity baseline (all four active) while
        // preventing phantom "missing analyst" failures for packs that
        // intentionally omit one. Each arm is still type-specialised because
        // each analyst writes a differently-shaped payload into state; the
        // registry-driven aggregation is the per-id gate here, not the types.
        if active_ids.contains(&AnalystId::Fundamental) {
            merge_analyst_result::<FundamentalData, _, _>(
                &context,
                &mut state,
                &mut failures,
                ANALYST_FUNDAMENTAL,
                |state, data| state.set_fundamental_metrics(data),
                |state, data| {
                    state.set_evidence_fundamental(EvidenceRecord {
                        kind: EvidenceKind::Fundamental,
                        payload: data,
                        sources: vec![stage1_source(
                            PROVIDER_FINNHUB,
                            vec!["fundamentals".to_owned()],
                        )],
                        quality_flags: vec![],
                    });
                },
            )
            .await;
        }
        if active_ids.contains(&AnalystId::Sentiment) {
            merge_analyst_result::<SentimentData, _, _>(
                &context,
                &mut state,
                &mut failures,
                ANALYST_SENTIMENT,
                |state, data| state.set_market_sentiment(data),
                |state, data| {
                    state.set_evidence_sentiment(EvidenceRecord {
                        kind: EvidenceKind::Sentiment,
                        payload: data,
                        sources: vec![stage1_source(
                            PROVIDER_FINNHUB,
                            vec!["company_news_sentiment_inputs".to_owned()],
                        )],
                        quality_flags: vec![],
                    });
                },
            )
            .await;
        }
        if active_ids.contains(&AnalystId::News) {
            merge_analyst_result::<NewsData, _, _>(
                &context,
                &mut state,
                &mut failures,
                ANALYST_NEWS,
                |state, data| state.set_macro_news(data),
                |state, data| {
                    state.set_evidence_news(EvidenceRecord {
                        kind: EvidenceKind::News,
                        payload: data,
                        sources: vec![
                            stage1_source(PROVIDER_FINNHUB, vec!["company_news".to_owned()]),
                            stage1_source(PROVIDER_FRED, vec!["macro_indicators".to_owned()]),
                        ],
                        quality_flags: vec![],
                    });
                },
            )
            .await;
        }
        if active_ids.contains(&AnalystId::Technical) {
            merge_analyst_result::<TechnicalData, _, _>(
                &context,
                &mut state,
                &mut failures,
                ANALYST_TECHNICAL,
                |state, data| state.set_technical_indicators(data),
                |state, data| {
                    let mut datasets = vec!["ohlcv".to_owned()];
                    if data.options_summary.is_some() {
                        datasets.push("options_snapshot".to_owned());
                    }
                    state.set_evidence_technical(EvidenceRecord {
                        kind: EvidenceKind::Technical,
                        payload: data,
                        sources: vec![stage1_source(PROVIDER_YFINANCE, datasets)],
                        quality_flags: vec![],
                    });
                },
            )
            .await;
        }

        let failure_count = failures.len();
        // Omission-aware degradation: the threshold is still "2+ failures
        // abort" per `check_analyst_degradation`, but the denominator is the
        // active analyst count, not a hard-coded 4. For the equity baseline
        // these are equivalent; for packs that omit an analyst, a single
        // failure out of three remains graceful-degradation territory.
        if failure_count >= 2 || (active_total > 0 && failure_count == active_total) {
            error!(
                failures = ?failures,
                active_total,
                "AnalystSyncTask: {failure_count}/{active_total} analysts failed — aborting pipeline"
            );
            return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                "AnalystSyncTask: {failure_count}/{active_total} analysts failed — pipeline aborted"
            )));
        }

        // Derive DataCoverageReport from the resolved runtime policy and the
        // presence/absence of the corresponding typed evidence fields.
        let required_inputs = required_inputs_for_state(&state);
        let missing_inputs: Vec<String> = required_inputs
            .iter()
            .filter(|input| input_missing(&state, input))
            .cloned()
            .collect();

        state.data_coverage = Some(DataCoverageReport {
            required_inputs,
            missing_inputs,
        });

        // Derive ProvenanceSummary from providers attached to present evidence records.
        let mut providers: Vec<String> = Vec::new();
        if let Some(rec) = state.evidence_fundamental() {
            providers.extend(rec.sources.iter().map(|s| s.provider.clone()));
        }
        if let Some(rec) = state.evidence_sentiment() {
            providers.extend(rec.sources.iter().map(|s| s.provider.clone()));
        }
        if let Some(rec) = state.evidence_news() {
            providers.extend(rec.sources.iter().map(|s| s.provider.clone()));
        }
        if let Some(rec) = state.evidence_technical() {
            providers.extend(rec.sources.iter().map(|s| s.provider.clone()));
        }
        providers.sort_unstable();
        providers.dedup();

        state.provenance_summary = Some(ProvenanceSummary {
            providers_used: providers,
        });

        // Derive deterministic valuation from Yahoo Finance financial statements.
        // All fetchers degrade gracefully to `None` on network failure — the cycle
        // must always continue regardless of availability.
        let symbol = state.asset_symbol.clone();
        let valuation_inputs =
            fetch_valuation_inputs(&self.yfinance, &symbol, self.valuation_fetch_timeout).await;
        let current_price = state.current_price;

        state.set_derived_valuation(derive_runtime_valuation(
            &state,
            &valuation_inputs,
            current_price,
        ));

        info!(
            task = "analyst_sync",
            asset_shape = ?state.derived_valuation().map(|d| &d.asset_shape),
            "deterministic valuation derived"
        );

        // Dynamic token accounting: one usage entry per active analyst so
        // phase totals reconcile against whatever fan-out the pack selected.
        let mut token_usages: Vec<AgentTokenUsage> = Vec::with_capacity(active_total);
        for id in &active_ids {
            token_usages
                .push(read_analyst_usage(&context, id.context_key(), id.display_name()).await);
        }
        let phase_duration_ms = token_usages
            .iter()
            .map(|usage| usage.latency_ms)
            .max()
            .unwrap_or(0);

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Analyst Fan-Out".to_owned(),
            phase_prompt_tokens: token_usages.iter().map(|usage| usage.prompt_tokens).sum(),
            phase_completion_tokens: token_usages
                .iter()
                .map(|usage| usage.completion_tokens)
                .sum(),
            phase_total_tokens: token_usages.iter().map(|usage| usage.total_tokens).sum(),
            phase_duration_ms,
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::sleep;

    use super::fetch_with_timeout;

    #[tokio::test]
    async fn fetch_with_timeout_preserves_fast_result_when_parallel_peer_times_out() {
        let timeout = Duration::from_millis(20);

        let (slow, fast) = tokio::join!(
            fetch_with_timeout::<&'static str, _>("AAPL", "slow", timeout, async {
                sleep(Duration::from_millis(40)).await;
                Some("slow")
            }),
            fetch_with_timeout::<&'static str, _>("AAPL", "fast", timeout, async { Some("fast") }),
        );

        assert_eq!(slow, None);
        assert_eq!(fast, Some("fast"));
    }
}
