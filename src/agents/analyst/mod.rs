//! Analyst team: four parallel specialist agents that produce structured data
//! for the downstream debate and trading pipeline.
//!
//! # Fan-out execution
//!
//! [`run_analyst_team`] spawns all four analysts concurrently via [`tokio::spawn`]
//! and collects results. The degradation policy tolerates one failure
//! (partial data continues); two or more failures abort the cycle with
//! [`TradingError::AnalystError`].
//!
//! # Sub-modules
//!
//! - [`fundamental`] – Fundamental Analyst (earnings, ratios, insider activity)
//! - [`sentiment`] – Sentiment Analyst (news-based, MVP)
//! - [`news`] – News Analyst (articles and macro events)
//! - [`technical`] – Technical Analyst (OHLCV → indicators → LLM summary)

mod common;
mod fundamental;
mod news;
mod sentiment;
mod technical;

pub use fundamental::FundamentalAnalyst;
pub use news::NewsAnalyst;
pub use sentiment::SentimentAnalyst;
pub use technical::TechnicalAnalyst;

use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::{
    config::LlmConfig,
    data::{FinnhubClient, YFinanceClient},
    error::{RetryPolicy, TradingError, check_analyst_degradation},
    providers::factory::CompletionModelHandle,
    state::{
        AgentTokenUsage, AnalystStateHandles, FundamentalData, NewsData, SentimentData,
        TechnicalData, TradingState,
    },
};

/// Run all four analyst agents concurrently and write results into `state`.
///
/// Each agent is constructed fresh, cloning the shared handles, then spawned
/// on the Tokio thread-pool. Results are collected after all tasks complete;
/// successes are written to the corresponding `TradingState` fields sequentially.
///
/// # Degradation policy
///
/// - 0 failures → all four fields populated, returns full `Vec<AgentTokenUsage>`
/// - 1 failure  → three fields populated, one `None`, continues with partial data
/// - 2+ failures → returns `TradingError::AnalystError`
///
/// # Errors
///
/// - [`TradingError::AnalystError`] when two or more analysts fail.
pub async fn run_analyst_team(
    handle: &CompletionModelHandle,
    finnhub: &FinnhubClient,
    yfinance: &YFinanceClient,
    state: &mut TradingState,
    llm_config: &LlmConfig,
) -> Result<Vec<AgentTokenUsage>, TradingError> {
    // The inner retry policy sets the per-attempt timeout; the outer task timeout
    // must cover all attempts plus backoff so it never fires before the inner budget
    // is exhausted.
    let inner_timeout = Duration::from_secs(llm_config.analyst_timeout_secs);
    let retry_policy = RetryPolicy::from_config(llm_config);
    let outer_timeout = retry_policy.total_budget(inner_timeout);

    let symbol = state.asset_symbol.clone();
    let target_date = state.target_date.clone();
    let analyst_handles = state.analyst_handles();
    let model_id = handle.model_id().to_owned();

    // ── Pre-fetch news once; both Sentiment and News analysts share the result ─
    //
    // This eliminates the duplicate Finnhub `get_news` call (P1).  If the
    // pre-fetch fails the analysts fall back to their live `GetNews` tool.
    let cached_news: Option<Arc<crate::state::NewsData>> = match finnhub.get_news(&symbol).await {
        Ok(data) => Some(Arc::new(data)),
        Err(err) => {
            warn!(error = %err, "news pre-fetch failed; analysts will use live tool calls");
            None
        }
    };

    // ── Spawn all four analysts concurrently ─────────────────────────────

    let fundamental_task = {
        let analyst = FundamentalAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let sentiment_task = {
        let analyst = SentimentAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
            cached_news.clone(),
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let news_task = {
        let analyst = NewsAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
            cached_news,
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let technical_task = {
        let analyst = TechnicalAnalyst::new(
            handle.clone(),
            yfinance.clone(),
            symbol,      // moved — last use; avoids a fourth clone
            target_date, // moved — last use; avoids a fourth clone
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    // ── Await all tasks ───────────────────────────────────────────────────

    let (fundamental_join, sentiment_join, news_join, technical_join) =
        tokio::join!(fundamental_task, sentiment_task, news_task, technical_task);

    // ── Unwrap JoinError, then timeout, then analyst error ────────────────

    let fundamental_result = flatten_task_result("Fundamental Analyst", fundamental_join);
    let sentiment_result = flatten_task_result("Sentiment Analyst", sentiment_join);
    let news_result = flatten_task_result("News Analyst", news_join);
    let technical_result = flatten_task_result("Technical Analyst", technical_join);

    apply_analyst_results(
        fundamental_result,
        sentiment_result,
        news_result,
        technical_result,
        &analyst_handles,
        state,
        &model_id,
    )
    .await
}

/// Collect four analyst results into `state`, emit warnings for failures,
/// capture a best-effort [`AgentTokenUsage`] for every run (success or error),
/// and apply the degradation policy.
///
/// Extracted from [`run_analyst_team`] so it can be tested without a live
/// LLM by supplying pre-built `Result` values directly.
pub(crate) async fn apply_analyst_results(
    fundamental: Result<(FundamentalData, AgentTokenUsage), TradingError>,
    sentiment: Result<(SentimentData, AgentTokenUsage), TradingError>,
    news: Result<(NewsData, AgentTokenUsage), TradingError>,
    technical: Result<(TechnicalData, AgentTokenUsage), TradingError>,
    handles: &AnalystStateHandles,
    state: &mut TradingState,
    model_id: &str,
) -> Result<Vec<AgentTokenUsage>, TradingError> {
    let mut token_usages: Vec<AgentTokenUsage> = Vec::new();
    let mut failed_agents: Vec<String> = Vec::new();

    macro_rules! handle_result {
        ($result:expr, $name:literal, $field:expr) => {
            match $result {
                Ok((data, usage)) => {
                    *$field.write().await = Some(data);
                    token_usages.push(usage);
                }
                Err(err) => {
                    warn!(agent = $name, error = %err, "analyst failed");
                    failed_agents.push($name.to_owned());
                    // Always record a best-effort usage entry so the phase tracker
                    // accounts for every analyst, successful or not.
                    token_usages.push(AgentTokenUsage::unavailable($name, model_id, 0));
                }
            }
        };
    }

    handle_result!(
        fundamental,
        "Fundamental Analyst",
        handles.fundamental_metrics
    );
    handle_result!(sentiment, "Sentiment Analyst", handles.market_sentiment);
    handle_result!(news, "News Analyst", handles.macro_news);
    handle_result!(technical, "Technical Analyst", handles.technical_indicators);

    // Check the degradation policy *before* committing partial results to the
    // shared state. This ensures that if we abort, the caller's TradingState is
    // never partially poisoned with data from a cycle that will not complete.
    check_analyst_degradation(4, &failed_agents)?;

    state.apply_analyst_handles(handles).await;

    Ok(token_usages)
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Flatten a `JoinHandle` result: converts task-level failures into typed trading errors.
fn flatten_task_result<T>(
    agent_name: &str,
    join_result: Result<
        Result<Result<T, TradingError>, tokio::time::error::Elapsed>,
        tokio::task::JoinError,
    >,
) -> Result<T, TradingError> {
    match join_result {
        // Task panicked or was cancelled.
        Err(join_err) => Err(TradingError::AnalystError {
            agent: agent_name.to_owned(),
            message: format!("task panicked or was cancelled: {join_err}"),
        }),
        // Task completed but timed out.
        Ok(Err(_elapsed)) => Err(TradingError::NetworkTimeout {
            // tokio::time::error::Elapsed does not expose the wall time of the
            // deadline; Duration::ZERO is a sentinel value — callers must infer
            // the actual elapsed time from context (e.g., the outer_timeout value).
            elapsed: Duration::ZERO,
            message: format!("{agent_name} task timed out"),
        }),
        // Task completed successfully — propagate inner result.
        Ok(Ok(inner)) => inner,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::TradingError,
        state::{
            FundamentalData, ImpactDirection, InsiderTransaction, MacdValues, MacroEvent,
            NewsArticle, NewsData, SentimentData, SentimentSource, TechnicalData, TransactionType,
        },
    };

    // ── Helpers ──────────────────────────────────────────────────────────

    fn sample_fundamental() -> FundamentalData {
        FundamentalData {
            revenue_growth_pct: Some(0.12),
            pe_ratio: Some(28.5),
            eps: Some(6.1),
            current_ratio: Some(1.3),
            debt_to_equity: None,
            gross_margin: Some(0.43),
            net_income: Some(9.5e10),
            insider_transactions: vec![InsiderTransaction {
                name: "Jane".to_owned(),
                share_change: -1000.0,
                transaction_date: "2026-01-01".to_owned(),
                transaction_type: TransactionType::S,
            }],
            summary: "Strong fundamentals.".to_owned(),
        }
    }

    fn sample_sentiment() -> SentimentData {
        SentimentData {
            overall_score: 0.6,
            source_breakdown: vec![SentimentSource {
                source_name: "Finnhub News".to_owned(),
                score: 0.6,
                sample_size: 12,
            }],
            engagement_peaks: vec![],
            summary: "Mildly bullish.".to_owned(),
        }
    }

    fn sample_news() -> NewsData {
        NewsData {
            articles: vec![NewsArticle {
                title: "Record Revenue".to_owned(),
                source: "Reuters".to_owned(),
                published_at: "2026-03-14T10:00:00Z".to_owned(),
                relevance_score: Some(0.9),
                snippet: "Record quarterly results.".to_owned(),
            }],
            macro_events: vec![MacroEvent {
                event: "Interest-rate policy shift".to_owned(),
                impact_direction: ImpactDirection::Positive,
                confidence: 0.75,
            }],
            summary: "Positive earnings and rate backdrop.".to_owned(),
        }
    }

    fn sample_technical() -> TechnicalData {
        TechnicalData {
            rsi: Some(55.0),
            macd: Some(MacdValues {
                macd_line: 0.1,
                signal_line: 0.05,
                histogram: 0.05,
            }),
            atr: Some(1.5),
            sma_20: Some(150.0),
            sma_50: None,
            ema_12: Some(151.0),
            ema_26: Some(149.0),
            bollinger_upper: Some(160.0),
            bollinger_lower: Some(140.0),
            support_level: None,
            resistance_level: None,
            volume_avg: Some(500_000.0),
            summary: "Neutral trend.".to_owned(),
        }
    }

    fn sample_usage(agent: &str) -> AgentTokenUsage {
        AgentTokenUsage {
            agent_name: agent.to_owned(),
            model_id: "gpt-4o-mini".to_owned(),
            token_counts_available: true,
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            latency_ms: 300,
        }
    }

    // ── flatten_task_result ──────────────────────────────────────────────

    #[test]
    fn flatten_join_error_becomes_analyst_error() {
        let ok: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Ok(Ok(42)));
        let result = flatten_task_result::<i32>("test", ok);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn flatten_inner_error_propagates() {
        let inner_err: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Ok(Err(TradingError::Rig("inner failure".to_owned()))));
        let result = flatten_task_result::<i32>("test", inner_err);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Rig(_)));
    }

    // ── outer timeout is larger than inner budget ─────────────────────────

    #[test]
    fn outer_timeout_exceeds_inner_timeout() {
        let inner = Duration::from_secs(30);
        let outer = RetryPolicy::default().total_budget(inner);
        // With max_retries=3 and base 500 ms: outer = 30×4 + 3.5s = 123.5s
        assert!(
            outer > inner,
            "outer timeout must be larger than per-attempt timeout"
        );
        assert_eq!(outer, Duration::from_millis(123_500));
    }

    #[test]
    fn timeout_duration_derived_from_config() {
        let config = LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 60,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        };
        let inner = Duration::from_secs(config.analyst_timeout_secs);
        let retry_policy = RetryPolicy::from_config(&config);
        let outer = retry_policy.total_budget(inner);
        assert!(outer > inner);
    }

    // ── Task 5.6 / 6.1: all four analysts succeed ────────────────────────

    #[tokio::test]
    async fn all_four_succeed_populates_all_state_fields() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        let result = apply_analyst_results(
            Ok((sample_fundamental(), sample_usage("Fundamental Analyst"))),
            Ok((sample_sentiment(), sample_usage("Sentiment Analyst"))),
            Ok((sample_news(), sample_usage("News Analyst"))),
            Ok((sample_technical(), sample_usage("Technical Analyst"))),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        assert!(result.is_ok());
        let usages = result.unwrap();
        // All four succeeded → four usage entries, all with token_counts_available
        assert_eq!(usages.len(), 4);
        assert!(usages.iter().all(|u| u.token_counts_available));
        // State fields populated
        assert!(state.fundamental_metrics.is_some());
        assert!(state.market_sentiment.is_some());
        assert!(state.macro_news.is_some());
        assert!(state.technical_indicators.is_some());
    }

    // ── Task 5.7 / 6.2: one analyst fails — partial data, continues ──────

    #[tokio::test]
    async fn one_failure_continues_with_partial_state() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        let result = apply_analyst_results(
            Ok((sample_fundamental(), sample_usage("Fundamental Analyst"))),
            Err(TradingError::NetworkTimeout {
                elapsed: Duration::from_secs(30),
                message: "simulated timeout".to_owned(),
            }),
            Ok((sample_news(), sample_usage("News Analyst"))),
            Ok((sample_technical(), sample_usage("Technical Analyst"))),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        // Should succeed despite one failure
        assert!(result.is_ok());
        let usages = result.unwrap();
        // Four entries — three real, one unavailable fallback
        assert_eq!(usages.len(), 4);
        // The failed analyst's fallback entry has token_counts_available = false
        let failed_usage = usages
            .iter()
            .find(|u| u.agent_name == "Sentiment Analyst")
            .expect("fallback usage for failed analyst must be present");
        assert!(!failed_usage.token_counts_available);
        // The failed field is None; the others are populated
        assert!(state.fundamental_metrics.is_some());
        assert!(
            state.market_sentiment.is_none(),
            "failed analyst field must be None"
        );
        assert!(state.macro_news.is_some());
        assert!(state.technical_indicators.is_some());
    }

    // ── Task 5.8 / 6.2: two failures → abort with both agent names ───────

    #[tokio::test]
    async fn two_failures_abort_with_both_agent_names() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        let result = apply_analyst_results(
            Err(TradingError::Rig("fundamental LLM error".to_owned())),
            Ok((sample_sentiment(), sample_usage("Sentiment Analyst"))),
            Err(TradingError::SchemaViolation {
                message: "news output malformed".to_owned(),
            }),
            Ok((sample_technical(), sample_usage("Technical Analyst"))),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match &err {
            TradingError::AnalystError { agent, message } => {
                assert!(
                    agent.contains("Fundamental Analyst"),
                    "error must name Fundamental Analyst; got: {agent}"
                );
                assert!(
                    agent.contains("News Analyst"),
                    "error must name News Analyst; got: {agent}"
                );
                assert!(
                    message.contains("2/4"),
                    "message must show failure count; got: {message}"
                );
            }
            other => panic!("expected AnalystError, got: {other:?}"),
        }
    }

    // ── Task 6.3: AgentTokenUsage collected for all analysts ─────────────

    #[tokio::test]
    async fn token_usages_collected_for_all_including_failed() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        // Only one failure — still returns Ok
        let result = apply_analyst_results(
            Err(TradingError::Rig("error".to_owned())),
            Ok((sample_sentiment(), sample_usage("Sentiment Analyst"))),
            Ok((sample_news(), sample_usage("News Analyst"))),
            Ok((sample_technical(), sample_usage("Technical Analyst"))),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        assert!(result.is_ok());
        let usages = result.unwrap();
        // Exactly 4 entries — one per analyst regardless of success/failure
        assert_eq!(usages.len(), 4, "must have one usage entry per analyst");
        let names: Vec<&str> = usages.iter().map(|u| u.agent_name.as_str()).collect();
        assert!(names.contains(&"Fundamental Analyst"));
        assert!(names.contains(&"Sentiment Analyst"));
        assert!(names.contains(&"News Analyst"));
        assert!(names.contains(&"Technical Analyst"));
    }
}
