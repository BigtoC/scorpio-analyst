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
    analysis_packs::PackId,
    config::LlmConfig,
    data::{
        FinnhubClient, FredClient, SecEdgarClient, YFinanceClient,
        adapters::transcripts::TranscriptFetch,
        sec_edgar_nport::NPortHoldings,
        yfinance::{
            Candle,
            etf::{EtfQuote, FundInfo, fund_info_from_profile, normalize_benchmark_symbol},
        },
    },
    providers::factory::CompletionModelHandle,
    state::{
        AgentTokenUsage, AssetShape, DataCoverageReport, DerivedValuation, EvidenceKind,
        EvidenceRecord, EvidenceSource, FundamentalData, NewsData, PhaseTokenUsage,
        ProvenanceSummary, ScenarioValuation, SentimentData, TechnicalData,
        TechnicalOptionsContext, TradingState, derive_valuation,
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
            ANALYST_TECHNICAL, OK_SUFFIX, load_transcript_fetch, read_analyst_usage,
            write_analyst_usage, write_err, write_flag,
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
/// Provider tag for earnings-call transcripts (Alpha Vantage).
const PROVIDER_ALPHA_VANTAGE: &str = "alpha_vantage";
/// Provider tag for SEC EDGAR catalyst feeds (8-K, etc.).
const PROVIDER_SEC_EDGAR: &str = "sec_edgar";
/// Provider tag for Reddit sentiment-sidecar rows.
const PROVIDER_REDDIT: &str = "reddit";

/// Source-tag used on `CatalystEvent.source` by [`crate::data::adapters::catalysts`]
/// when an 8-K filing comes from SEC EDGAR. Kept in sync there manually — this is
/// a string contract between producer and aggregator.
const CATALYST_SOURCE_SEC_EDGAR: &str = "sec_edgar";

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

#[cfg(test)]
mod reddit_lane_tests {
    use super::*;
    use crate::state::NewsArticle;
    use crate::workflow::tasks::{KEY_CACHED_SENTIMENT_NEWS, KEY_CACHED_VETTED_NEWS};

    #[tokio::test]
    async fn sentiment_cache_contains_reddit_detects_sidecar_rows() {
        let context = Context::new();
        context
            .set(
                KEY_CACHED_SENTIMENT_NEWS,
                serde_json::to_string(&NewsData {
                    articles: vec![NewsArticle {
                        title: "Retail chatter".to_owned(),
                        source: "Reddit r/stocks".to_owned(),
                        published_at: "2026-03-19T12:00:00Z".to_owned(),
                        relevance_score: None,
                        snippet: String::new(),
                        url: None,
                    }],
                    macro_events: vec![],
                    summary: "reddit sidecar".to_owned(),
                })
                .expect("serialize sentiment cache"),
            )
            .await;

        let sources = sentiment_evidence_sources(&context)
            .await
            .expect("cache read should succeed");

        assert!(
            sources
                .iter()
                .any(|source| source.provider == PROVIDER_REDDIT),
            "Reddit sidecar rows should be detected from the sentiment cache"
        );
    }

    #[tokio::test]
    async fn read_cached_news_at_reads_lane_specific_keys() {
        let context = Context::new();
        context
            .set(
                KEY_CACHED_VETTED_NEWS,
                serde_json::to_string(&NewsData {
                    articles: vec![NewsArticle {
                        title: "Wire".to_owned(),
                        source: "Reuters".to_owned(),
                        published_at: "2026-03-19T12:00:00Z".to_owned(),
                        relevance_score: None,
                        snippet: String::new(),
                        url: None,
                    }],
                    macro_events: vec![],
                    summary: "vetted".to_owned(),
                })
                .expect("serialize vetted cache"),
            )
            .await;
        context
            .set(
                KEY_CACHED_SENTIMENT_NEWS,
                serde_json::to_string(&NewsData {
                    articles: vec![NewsArticle {
                        title: "Crowd".to_owned(),
                        source: "Reddit r/stocks".to_owned(),
                        published_at: "2026-03-19T11:00:00Z".to_owned(),
                        relevance_score: None,
                        snippet: String::new(),
                        url: None,
                    }],
                    macro_events: vec![],
                    summary: "sentiment".to_owned(),
                })
                .expect("serialize sentiment cache"),
            )
            .await;

        let vetted = read_cached_news_at("test", &context, KEY_CACHED_VETTED_NEWS)
            .await
            .expect("vetted cache read")
            .expect("vetted cache present");
        let sentiment = read_cached_news_at("test", &context, KEY_CACHED_SENTIMENT_NEWS)
            .await
            .expect("sentiment cache read")
            .expect("sentiment cache present");

        assert_eq!(vetted.articles.len(), 1);
        assert_eq!(vetted.articles[0].source, "Reuters");
        assert_eq!(sentiment.articles.len(), 1);
        assert!(sentiment.articles[0].source.starts_with("Reddit r/"));
    }
}

async fn read_cached_news_at(
    task_name: &str,
    context: &Context,
    key: &str,
) -> graph_flow::Result<Option<Arc<NewsData>>> {
    let json: Option<String> = context.get(key).await;
    json.map(|value| {
        serde_json::from_str::<NewsData>(&value).map(Arc::new).map_err(|error| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "{task_name}: orchestration corruption: cached news deserialization failed: {error}"
            ))
        })
    })
    .transpose()
}

async fn sentiment_evidence_sources(context: &Context) -> graph_flow::Result<Vec<EvidenceSource>> {
    let Some(news) =
        read_cached_news_at("AnalystSyncTask", context, super::KEY_CACHED_SENTIMENT_NEWS).await?
    else {
        return Ok(vec![stage1_source(
            PROVIDER_FINNHUB,
            vec!["company_news_sentiment_inputs".to_owned()],
        )]);
    };

    let has_reddit = news
        .articles
        .iter()
        .any(|article| article.source.starts_with("Reddit r/"));
    let has_non_reddit = news
        .articles
        .iter()
        .any(|article| !article.source.starts_with("Reddit r/"));

    let mut sources = Vec::new();
    if has_non_reddit || news.articles.is_empty() {
        sources.push(stage1_source(
            PROVIDER_FINNHUB,
            vec!["company_news_sentiment_inputs".to_owned()],
        ));
    }
    if has_reddit {
        sources.push(stage1_source(
            PROVIDER_REDDIT,
            vec!["crowd_commentary_sentiment_inputs".to_owned()],
        ));
    }
    if sources.is_empty() {
        sources.push(stage1_source(
            PROVIDER_FINNHUB,
            vec!["company_news_sentiment_inputs".to_owned()],
        ));
    }
    Ok(sources)
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

        let cached_news_opt = read_cached_news_at(
            "SentimentAnalystTask",
            &context,
            super::KEY_CACHED_SENTIMENT_NEWS,
        )
        .await?;

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "SentimentAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let transcript_fetch = load_transcript_fetch(&context).await.map_err(|error| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "SentimentAnalystTask: orchestration corruption: \
                 transcript fetch status unreadable: {error}"
            ))
        })?;

        let analyst = SentimentAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            &state,
            policy,
            &self.llm_config,
            cached_news_opt,
            Some(transcript_fetch),
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

        let cached_news_opt =
            read_cached_news_at("NewsAnalystTask", &context, super::KEY_CACHED_VETTED_NEWS).await?;

        let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
            graph_flow::GraphError::TaskExecutionFailed(
                "NewsAnalystTask: orchestration corruption: \
                 state.analysis_runtime_policy is missing — preflight is the sole writer \
                 and must run before analyst fan-out"
                    .to_owned(),
            )
        })?;

        let transcript_fetch = load_transcript_fetch(&context).await.map_err(|error| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "NewsAnalystTask: orchestration corruption: \
                 transcript fetch status unreadable: {error}"
            ))
        })?;

        let analyst = NewsAnalyst::new(
            self.handle.clone(),
            self.finnhub.clone(),
            self.fred.clone(),
            &state,
            policy,
            &self.llm_config,
            cached_news_opt,
            Some(transcript_fetch),
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
    /// Optional SEC EDGAR client. Consumed by the ETF input hydration path
    /// (Task 13) to fetch N-PORT-P holdings when the active pack is
    /// `EtfBaseline`. Left `None` for the equity baseline and for tests that
    /// don't need EDGAR.
    sec_edgar: Option<Arc<SecEdgarClient>>,
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
            sec_edgar: None,
            valuation_fetch_timeout,
        })
    }

    /// Create a new `AnalystSyncTask` with both Yahoo Finance and SEC EDGAR
    /// clients.
    ///
    /// The SEC EDGAR client is used by the ETF pack to fetch N-PORT-P
    /// holdings for premium/discount valuation. It is wrapped in `Option` on
    /// the struct so non-ETF runs (and tests that don't need it) can leave
    /// it unset via [`AnalystSyncTask::with_yfinance`] without breaking
    /// graceful degradation.
    #[must_use]
    pub fn with_yfinance_and_edgar(
        snapshot_store: Arc<SnapshotStore>,
        yfinance: YFinanceClient,
        sec_edgar: Arc<SecEdgarClient>,
        valuation_fetch_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            snapshot_store,
            yfinance,
            sec_edgar: Some(sec_edgar),
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
    // ETF inputs — populated only when pack == EtfBaseline.
    etf_quote: Option<EtfQuote>,
    etf_fund_info: Option<FundInfo>,
    etf_holdings: Option<NPortHoldings>,
    etf_ohlcv: Option<Vec<Candle>>,
    etf_benchmark_ohlcv: Option<Vec<Candle>>,
    /// Cached TTM distribution yield (from yfinance), used to fill
    /// `EtfComposition.distribution_yield_ttm_pct` after the valuator returns.
    etf_distribution_yield_ttm_pct: Option<f64>,
}

/// Trailing window for ETF / benchmark OHLCV fetches in [`fetch_valuation_inputs`].
///
/// One year is sufficient for the 90d and 1y tracking-error windows computed
/// by [`crate::valuation::EtfPremiumDiscountValuator`].
const ETF_OHLCV_WINDOW_DAYS: i64 = 365;

/// Fetch an OHLCV series for the last [`ETF_OHLCV_WINDOW_DAYS`] days.
///
/// Wraps the `Result`-returning [`YFinanceClient::get_ohlcv`] into an
/// `Option`-returning future so it can flow through [`fetch_with_timeout`]
/// alongside the other fail-soft ETF fetches. Date-range failures or
/// transport errors degrade to `None` rather than propagating.
async fn fetch_ohlcv_1y(yfinance: &YFinanceClient, symbol: &str) -> Option<Vec<Candle>> {
    let today = chrono::Utc::now().date_naive();
    let start = today - chrono::Duration::days(ETF_OHLCV_WINDOW_DAYS);
    yfinance
        .get_ohlcv(symbol, &start.to_string(), &today.to_string())
        .await
        .ok()
}

async fn fetch_valuation_inputs(
    yfinance: &YFinanceClient,
    sec_edgar: Option<&Arc<SecEdgarClient>>,
    pack_id: PackId,
    symbol: &str,
    target_date: &str,
    fetch_timeout: Duration,
) -> ValuationInputs {
    let profile = fetch_with_timeout(
        symbol,
        "profile",
        fetch_timeout,
        yfinance.get_profile(symbol),
    )
    .await;
    let (cashflow, balance, income, shares, trend) = if pack_id == PackId::EtfBaseline {
        (None, None, None, None, None)
    } else {
        tokio::join!(
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
        )
    };

    let mut etf_quote = None;
    let mut etf_fund_info = None;
    let mut etf_holdings = None;
    let mut etf_ohlcv = None;
    let mut etf_benchmark_ohlcv = None;
    let mut etf_distribution_yield_ttm_pct = None;

    if pack_id == PackId::EtfBaseline {
        let is_historical_target_date = target_date
            != chrono::Utc::now()
                .date_naive()
                .format("%Y-%m-%d")
                .to_string();

        if is_historical_target_date {
            return ValuationInputs {
                profile,
                cashflow,
                balance,
                income,
                shares,
                trend,
                etf_quote,
                etf_fund_info,
                etf_holdings,
                etf_ohlcv,
                etf_benchmark_ohlcv,
                etf_distribution_yield_ttm_pct,
            };
        }

        // Parallel ETF fetches that don't depend on each other.
        let fund_info_from_profile = profile
            .as_ref()
            .and_then(|profile| fund_info_from_profile(symbol, profile));
        let (quote_opt, info_opt, yld_opt, etf_ohlcv_opt) = tokio::join!(
            fetch_with_timeout(
                symbol,
                "etf_quote",
                fetch_timeout,
                yfinance.get_quote(symbol)
            ),
            fetch_with_timeout(
                symbol,
                "etf_fund_info",
                fetch_timeout,
                yfinance.get_fund_info(symbol),
            ),
            fetch_with_timeout(
                symbol,
                "etf_dist_yield",
                fetch_timeout,
                yfinance.get_distribution_yield_ttm(symbol),
            ),
            fetch_with_timeout(
                symbol,
                "etf_ohlcv",
                fetch_timeout,
                fetch_ohlcv_1y(yfinance, symbol),
            ),
        );
        etf_quote = quote_opt;
        etf_fund_info = info_opt.or(fund_info_from_profile);
        etf_distribution_yield_ttm_pct = yld_opt;
        etf_ohlcv = etf_ohlcv_opt;

        // Sequential N-PORT-P fetch (depends on CIK resolution).
        if let Some(edgar) = sec_edgar
            && let Some(cik) = fetch_with_timeout(
                symbol,
                "fund_cik",
                fetch_timeout,
                edgar.resolve_fund_cik(symbol),
            )
            .await
        {
            etf_holdings = fetch_with_timeout(
                symbol,
                "nport_holdings",
                fetch_timeout,
                edgar.fetch_latest_nport_p(&cik, 180),
            )
            .await;
        }

        // Benchmark OHLCV depends on the stated benchmark symbol pulled from
        // fund_info — kept sequential to avoid issuing a phantom fetch when
        // the benchmark is unknown.
        if let Some(bench) = resolve_benchmark_symbol(etf_fund_info.as_ref(), etf_holdings.as_ref())
        {
            etf_benchmark_ohlcv = fetch_with_timeout(
                symbol,
                "etf_benchmark_ohlcv",
                fetch_timeout,
                fetch_ohlcv_1y(yfinance, &bench),
            )
            .await;
        }
    }

    ValuationInputs {
        profile,
        cashflow,
        balance,
        income,
        shares,
        trend,
        etf_quote,
        etf_fund_info,
        etf_holdings,
        etf_ohlcv,
        etf_benchmark_ohlcv,
        etf_distribution_yield_ttm_pct,
    }
}

fn resolve_benchmark_symbol(
    fund_info: Option<&FundInfo>,
    nport: Option<&NPortHoldings>,
) -> Option<String> {
    fund_info
        .and_then(|info| info.stated_benchmark.as_deref())
        .and_then(normalize_benchmark_symbol)
        .or_else(|| {
            nport
                .and_then(|holdings| holdings.stated_benchmark.as_deref())
                .and_then(normalize_benchmark_symbol)
        })
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

/// Extract the live ETF options snapshot from persisted technical state.
///
/// Returns `Some(&snapshot)` only when `TechnicalOptionsContext::Available`
/// carries an `OptionsOutcome::Snapshot(_)`. Every other variant emits a
/// `tracing::warn!` and returns `None` so the valuator leaves
/// dealer-positioning absent cleanly.
pub(crate) fn etf_options_from_state(
    state: &crate::state::TradingState,
) -> Option<&crate::data::traits::options::OptionsSnapshot> {
    use crate::data::traits::options::OptionsOutcome;
    use crate::state::TechnicalOptionsContext;

    let technical = state.technical_indicators()?;
    let options_context = technical.options_context.as_ref()?;
    match options_context {
        TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(snap),
        } => Some(snap),
        TechnicalOptionsContext::Available { outcome: other } => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                outcome = %other,
                symbol = %state.asset_symbol,
                "ETF options chain unavailable — dealer positioning skipped"
            );
            None
        }
        TechnicalOptionsContext::FetchFailed { reason } => {
            tracing::warn!(
                target: "scorpio_core::workflow::analyst",
                symbol = %state.asset_symbol,
                fetch_reason = %reason,
                "ETF options fetch failed before valuation — dealer positioning skipped"
            );
            None
        }
    }
}

pub(crate) fn etf_risk_free_rate_from_state(state: &crate::state::TradingState) -> Option<f64> {
    state.etf_risk_free_rate
}

fn derive_runtime_valuation(
    state: &TradingState,
    valuation_inputs: &ValuationInputs,
    current_price: Option<f64>,
) -> DerivedValuation {
    let mut etf_fund_info = valuation_inputs.etf_fund_info.clone();
    if let Some(benchmark_symbol) = resolve_benchmark_symbol(
        valuation_inputs.etf_fund_info.as_ref(),
        valuation_inputs.etf_holdings.as_ref(),
    ) && let Some(info) = etf_fund_info.as_mut()
        && info.stated_benchmark.is_none()
    {
        info.stated_benchmark = Some(benchmark_symbol);
    }

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

    let registry = match policy.pack_id {
        PackId::EtfBaseline => ValuatorRegistry::etf_baseline(),
        _ => ValuatorRegistry::equity_baseline(),
    };
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
            etf_quote: valuation_inputs.etf_quote.as_ref(),
            etf_fund_info: etf_fund_info.as_ref(),
            etf_holdings: valuation_inputs.etf_holdings.as_ref(),
            etf_ohlcv: valuation_inputs.etf_ohlcv.as_deref(),
            etf_benchmark_ohlcv: valuation_inputs.etf_benchmark_ohlcv.as_deref(),
            etf_options: etf_options_from_state(state),
            etf_risk_free_rate: etf_risk_free_rate_from_state(state),
            as_of: chrono::NaiveDate::parse_from_str(&state.target_date, "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Utc::now().date_naive()),
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

        // Did enrichment-layer providers actually contribute to this cycle?
        // Computed once here so we can append their `EvidenceSource`s to the
        // existing analyst evidence records below — keeping the producer-side
        // attribution model that the rest of the aggregator already follows.
        let transcript_from_alpha_vantage = matches!(
            load_transcript_fetch(&context).await.ok(),
            Some(TranscriptFetch::Found(_))
        );
        let sentiment_sources = sentiment_evidence_sources(&context).await?;
        let sec_edgar_contributed_catalysts = state
            .enrichment_catalysts
            .payload
            .as_ref()
            .is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event.source == CATALYST_SOURCE_SEC_EDGAR)
            });

        // Only merge for analysts the active pack declared — keeps byte-identical
        // behaviour for the equity baseline (all four active) while
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
                    let mut sources = sentiment_sources.clone();
                    if transcript_from_alpha_vantage {
                        sources.push(stage1_source(
                            PROVIDER_ALPHA_VANTAGE,
                            vec!["earnings_transcript".to_owned()],
                        ));
                    }
                    state.set_evidence_sentiment(EvidenceRecord {
                        kind: EvidenceKind::Sentiment,
                        payload: data,
                        sources,
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
                    let mut sources = vec![
                        stage1_source(PROVIDER_FINNHUB, vec!["company_news".to_owned()]),
                        stage1_source(PROVIDER_FRED, vec!["macro_indicators".to_owned()]),
                    ];
                    if transcript_from_alpha_vantage {
                        sources.push(stage1_source(
                            PROVIDER_ALPHA_VANTAGE,
                            vec!["earnings_transcript".to_owned()],
                        ));
                    }
                    if sec_edgar_contributed_catalysts {
                        sources.push(stage1_source(
                            PROVIDER_SEC_EDGAR,
                            vec!["form_8k".to_owned()],
                        ));
                    }
                    state.set_evidence_news(EvidenceRecord {
                        kind: EvidenceKind::News,
                        payload: data,
                        sources,
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
                    // `fetched_at` is a coarse cycle anchor: options prefetch and OHLCV
                    // tool calls are temporally decoupled within the technical run.
                    if matches!(
                        data.options_context,
                        Some(TechnicalOptionsContext::Available { .. })
                    ) {
                        datasets.push("options_context".to_owned());
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
        let pack_id = state
            .analysis_runtime_policy
            .as_ref()
            .map_or(PackId::Baseline, |p| p.pack_id);
        let valuation_inputs = fetch_valuation_inputs(
            &self.yfinance,
            self.sec_edgar.as_ref(),
            pack_id,
            &symbol,
            &state.target_date,
            self.valuation_fetch_timeout,
        )
        .await;
        let current_price = state.current_price;

        state.set_derived_valuation(derive_runtime_valuation(
            &state,
            &valuation_inputs,
            current_price,
        ));

        // Post-process the ETF valuation: the valuator can't fetch the
        // distribution yield itself (the dividend-history path lives behind
        // `YFinanceClient` and is not exposed to valuators), so we attach it
        // here once the composition snapshot has been written.
        if let Some(yld) = valuation_inputs.etf_distribution_yield_ttm_pct
            && let Some(dv) = state.derived_valuation_mut()
            && let ScenarioValuation::Etf(etf) = &mut dv.scenario
            && let Some(comp) = etf.composition.as_mut()
        {
            comp.distribution_yield_ttm_pct = Some(yld);
        }

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

    use chrono::{NaiveDate, Utc};
    use tokio::time::sleep;
    use yfinance_rs::profile::{Fund, Profile};

    use super::{ValuationInputs, fetch_valuation_inputs, fetch_with_timeout};
    use crate::analysis_packs::PackId;
    use crate::data::yfinance::{Candle, etf::FundInfo};
    use crate::data::{StubbedFinancialResponses, YFinanceClient, sec_edgar_nport::NPortHoldings};
    use crate::valuation::{ValuatorId, ValuatorRegistry};

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

    /// Mirrors the registry switch inside `derive_runtime_valuation` to lock
    /// in that `PackId::EtfBaseline` resolves to a registry that knows about
    /// the ETF premium/discount valuator. The classifier wiring that actually
    /// flips this at runtime lands in a later task — this test guards the
    /// dispatch logic in isolation.
    #[test]
    fn etf_routing_selects_etf_baseline_registry() {
        let pack_id = PackId::EtfBaseline;
        let registry = match pack_id {
            PackId::EtfBaseline => ValuatorRegistry::etf_baseline(),
            _ => ValuatorRegistry::equity_baseline(),
        };
        assert!(registry.get(ValuatorId::EtfPremiumDiscount).is_some());
    }

    #[test]
    fn baseline_routing_falls_back_to_equity_registry_without_etf_valuator() {
        let pack_id = PackId::Baseline;
        let registry = match pack_id {
            PackId::EtfBaseline => ValuatorRegistry::etf_baseline(),
            _ => ValuatorRegistry::equity_baseline(),
        };
        assert!(registry.get(ValuatorId::EtfPremiumDiscount).is_none());
    }

    #[tokio::test]
    async fn etf_baseline_fetch_skips_equity_statement_fanout() {
        let yfinance = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            profile: Some(Profile::Fund(Fund {
                name: "SPDR S&P 500 ETF Trust".to_owned(),
                family: Some("State Street".to_owned()),
                kind: Default::default(),
                isin: None,
            })),
            ..StubbedFinancialResponses::default()
        });
        let today = chrono::Utc::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();

        let inputs = fetch_valuation_inputs(
            &yfinance,
            None,
            PackId::EtfBaseline,
            "SPY",
            &today,
            Duration::from_secs(1),
        )
        .await;

        assert!(
            inputs.profile.is_some(),
            "asset-shape detection should still fetch profile"
        );
        assert!(
            inputs.cashflow.is_none(),
            "ETF pack should not fetch equity cashflow"
        );
        assert!(
            inputs.balance.is_none(),
            "ETF pack should not fetch equity balance sheet"
        );
        assert!(
            inputs.income.is_none(),
            "ETF pack should not fetch equity income statement"
        );
        assert!(
            inputs.shares.is_none(),
            "ETF pack should not fetch equity shares"
        );
        assert!(
            inputs.trend.is_none(),
            "ETF pack should not fetch equity earnings trend"
        );
    }

    #[tokio::test]
    async fn etf_baseline_historical_target_date_skips_live_etf_fetches() {
        let yfinance = YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            profile: Some(Profile::Fund(Fund {
                name: "SPDR S&P 500 ETF Trust".to_owned(),
                family: Some("State Street".to_owned()),
                kind: Default::default(),
                isin: None,
            })),
            ..StubbedFinancialResponses::default()
        });

        let inputs = fetch_valuation_inputs(
            &yfinance,
            None,
            PackId::EtfBaseline,
            "SPY",
            "2024-01-15",
            Duration::from_secs(1),
        )
        .await;

        assert!(
            inputs.profile.is_some(),
            "asset-shape detection should still fetch profile"
        );
        assert!(inputs.etf_quote.is_none());
        assert!(inputs.etf_fund_info.is_none());
        assert!(inputs.etf_holdings.is_none());
        assert!(inputs.etf_ohlcv.is_none());
        assert!(inputs.etf_benchmark_ohlcv.is_none());
        assert!(inputs.etf_distribution_yield_ttm_pct.is_none());
    }

    #[test]
    fn benchmark_symbol_prefers_fund_info_then_nport_fallback() {
        let fund_info = FundInfo {
            symbol: "SPY".into(),
            category: None,
            fund_family: None,
            expense_ratio: None,
            total_assets: None,
            leverage_factor: Some(1.0),
            fund_kind: Some("etf".into()),
            stated_benchmark: None,
        };
        let nport = NPortHoldings {
            filing_date: NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            holdings: vec![],
            sector_breakdown: vec![],
            stated_benchmark: Some("S&P 500 Index".into()),
        };

        assert_eq!(
            super::resolve_benchmark_symbol(Some(&fund_info), Some(&nport)),
            Some("^GSPC".to_owned())
        );
    }

    #[test]
    fn benchmark_symbol_prefers_fund_info_when_present() {
        let fund_info = FundInfo {
            symbol: "SPY".into(),
            category: None,
            fund_family: None,
            expense_ratio: None,
            total_assets: None,
            leverage_factor: Some(1.0),
            fund_kind: Some("etf".into()),
            stated_benchmark: Some("^NDX".into()),
        };
        let nport = NPortHoldings {
            filing_date: NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            holdings: vec![],
            sector_breakdown: vec![],
            stated_benchmark: Some("S&P 500 Index".into()),
        };

        assert_eq!(
            super::resolve_benchmark_symbol(Some(&fund_info), Some(&nport)),
            Some("^NDX".to_owned())
        );
    }

    #[test]
    fn derive_runtime_valuation_uses_nport_benchmark_fallback() {
        let mut state = crate::state::TradingState::new("SPY", "2026-02-01");
        state.analysis_runtime_policy = Some(
            crate::analysis_packs::resolve_runtime_policy_for_manifest(
                &crate::analysis_packs::resolve_pack(crate::analysis_packs::PackId::EtfBaseline),
            )
            .expect("etf baseline policy"),
        );

        let quote = crate::data::yfinance::etf::EtfQuote {
            symbol: "SPY".into(),
            regular_market_price: 600.0,
            previous_close: None,
            nav: Some(599.5),
            bid: Some(599.8),
            ask: Some(600.2),
            market_cap: None,
            day_volume: None,
            currency: Some("USD".into()),
            as_of: Utc::now(),
        };
        let fund_info = FundInfo {
            symbol: "SPY".into(),
            category: Some("Large Blend".into()),
            fund_family: None,
            expense_ratio: Some(0.09),
            total_assets: None,
            leverage_factor: Some(1.0),
            fund_kind: Some("etf".into()),
            stated_benchmark: None,
        };
        let nport = NPortHoldings {
            filing_date: NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            holdings: vec![],
            sector_breakdown: vec![],
            stated_benchmark: Some("S&P 500 Index".into()),
        };
        let etf_ohlcv: Vec<Candle> = (0..35)
            .map(|i| Candle {
                date: format!("2026-01-{:02}", i + 1),
                open: 100.0 + i as f64,
                high: 100.0 + i as f64,
                low: 100.0 + i as f64,
                close: 100.0 + i as f64,
                volume: None,
            })
            .collect();
        let benchmark_ohlcv: Vec<Candle> = (0..35)
            .map(|i| Candle {
                date: format!("2026-01-{:02}", i + 1),
                open: 200.0 + i as f64,
                high: 200.0 + i as f64,
                low: 200.0 + i as f64,
                close: 200.0 + i as f64,
                volume: None,
            })
            .collect();

        let valuation_inputs = ValuationInputs {
            profile: Some(Profile::Fund(Fund {
                name: "SPDR S&P 500 ETF Trust".to_owned(),
                family: Some("State Street".to_owned()),
                kind: Default::default(),
                isin: None,
            })),
            cashflow: None,
            balance: None,
            income: None,
            shares: None,
            trend: None,
            etf_quote: Some(quote),
            etf_fund_info: Some(fund_info),
            etf_holdings: Some(nport),
            etf_ohlcv: Some(etf_ohlcv),
            etf_benchmark_ohlcv: Some(benchmark_ohlcv),
            etf_distribution_yield_ttm_pct: None,
        };

        let valuation = super::derive_runtime_valuation(&state, &valuation_inputs, Some(600.0));

        let tracking_symbol = match valuation.scenario {
            crate::state::ScenarioValuation::Etf(etf) => {
                etf.tracking
                    .expect("tracking should be computed with fallback benchmark")
                    .benchmark_symbol
            }
            other => panic!("expected ETF valuation, got {other:?}"),
        };

        assert_eq!(tracking_symbol, "^GSPC");
    }

    #[test]
    fn etf_valuation_inputs_carry_options_when_technical_context_has_snapshot() {
        use crate::data::traits::options::{
            IvTermPoint, NearTermStrike, OptionsOutcome, OptionsSnapshot,
        };
        use crate::state::{TechnicalData, TechnicalOptionsContext};

        let snap = OptionsSnapshot {
            spot_price: 100.0,
            atm_iv: 0.20,
            iv_term_structure: vec![IvTermPoint {
                expiration: "2026-06-26".to_owned(),
                atm_iv: 0.20,
            }],
            put_call_volume_ratio: 1.0,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 100.0,
            near_term_expiration: "2026-06-26".to_owned(),
            near_term_strikes: vec![NearTermStrike {
                strike: 100.0,
                call_iv: Some(0.20),
                put_iv: Some(0.20),
                call_volume: None,
                put_volume: None,
                call_oi: Some(1_000),
                put_oi: Some(1_000),
            }],
            all_expirations: vec![],
        };

        let mut state = crate::state::TradingState::new("SPY", "2026-06-01");
        crate::testing::with_baseline_runtime_policy(&mut state);
        state.set_technical_indicators(TechnicalData {
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
            summary: "smoke".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(snap.clone()),
            }),
        });

        let extracted = super::etf_options_from_state(&state);
        assert!(matches!(extracted, Some(s) if s.spot_price == snap.spot_price));
    }

    #[test]
    fn etf_valuation_inputs_drop_options_when_technical_context_is_fetch_failed() {
        use crate::state::{TechnicalData, TechnicalOptionsContext};

        let mut state = crate::state::TradingState::new("SPY", "2026-06-01");
        crate::testing::with_baseline_runtime_policy(&mut state);
        state.set_technical_indicators(TechnicalData {
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
            summary: "smoke".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::FetchFailed {
                reason: "connection refused".to_owned(),
            }),
        });

        let extracted = super::etf_options_from_state(&state);
        assert!(extracted.is_none());
    }

    #[test]
    fn etf_valuation_inputs_thread_etf_risk_free_rate_from_state() {
        let mut state = crate::state::TradingState::new("SPY", "2026-06-01");
        crate::testing::with_baseline_runtime_policy(&mut state);

        state.etf_risk_free_rate = Some(0.0427);
        let inputs_rate = super::etf_risk_free_rate_from_state(&state);
        assert_eq!(inputs_rate, Some(0.0427));

        state.etf_risk_free_rate = None;
        let inputs_rate_none = super::etf_risk_free_rate_from_state(&state);
        assert!(inputs_rate_none.is_none());
    }
}
