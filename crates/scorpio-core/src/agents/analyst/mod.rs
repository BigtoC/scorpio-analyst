//! Analyst team: specialist agents that produce structured data for the
//! downstream debate and trading pipeline.
//!
//! # Fan-out execution
//!
//! [`run_analyst_team`] spawns the equity analyst set concurrently via
//! [`tokio::spawn`] and collects results. The degradation policy tolerates
//! one failure (partial data continues); two or more failures abort the
//! cycle with [`TradingError::AnalystError`].
//!
//! # Module layout
//!
//! - [`traits`] — [`Analyst`], [`AnalystId`], [`DataNeed`] (shared across
//!   asset-class packs).
//! - [`registry`] — [`AnalystRegistry`] catalog of analysts the pipeline
//!   can dispatch to, consumed by `workflow/pipeline/runtime::build_graph`.
//! - [`equity`] — the four analysts that ship today
//!   ([`FundamentalAnalyst`], [`SentimentAnalyst`], [`NewsAnalyst`],
//!   [`TechnicalAnalyst`]).
//! - [`crypto`] — empty stubs for the crypto pack slice; none are wired
//!   into a live graph in this refactor.

pub mod crypto;
pub mod equity;
pub mod registry;
pub mod traits;

pub use equity::{FundamentalAnalyst, NewsAnalyst, SentimentAnalyst, TechnicalAnalyst};
pub use registry::AnalystRegistry;
pub use traits::{Analyst, AnalystId, DataNeed};

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

use crate::{
    config::LlmConfig,
    data::{FinnhubClient, FredClient, YFinanceClient, YFinanceNewsProvider, traits::NewsProvider},
    domain::{Symbol, Ticker},
    error::{RetryPolicy, TradingError, check_analyst_degradation},
    providers::factory::CompletionModelHandle,
    state::{
        AgentTokenUsage, AnalystStateHandles, FundamentalData, NewsArticle, NewsData,
        SentimentData, TechnicalData, TradingState,
    },
};

/// Maximum number of articles kept after merging Finnhub and Yahoo news.
const NEWS_PREFETCH_MAX_ARTICLES: usize = 20;

// ─── URL canonicalization ─────────────────────────────────────────────────────

/// Known URL shortener hosts — treated as "no URL" for dedup purposes so they
/// fall back to title-hash deduplication instead of matching the full canonical
/// URL from the other provider.
const SHORTENER_HOSTS: &[&str] = &[
    "yhoo.it",
    "bit.ly",
    "t.co",
    "tinyurl.com",
    "ow.ly",
    "goo.gl",
];

/// Canonicalize a URL for deduplication.
///
/// Returns `None` when the URL belongs to a known shortener (so callers fall
/// back to title-hash dedup) or when the input is empty/whitespace. Otherwise
/// returns a normalized form: scheme+host+path in lowercase, trailing slashes
/// stripped, and `?utm_*` query parameters removed.
fn canonical_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // Strip the scheme to reach the host+path portion.
    let without_scheme = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        url
    };

    // Extract just the host (up to the first `/` or `?`).
    let host_end = without_scheme
        .find(['/', '?'])
        .unwrap_or(without_scheme.len());
    let host = &without_scheme[..host_end];

    // Reject known shorteners.
    let host_lower = host.to_lowercase();
    if SHORTENER_HOSTS.iter().any(|&s| host_lower == s) {
        return None;
    }

    // Split off query string.
    let (path_part, query_part) = match without_scheme.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (without_scheme, None),
    };

    // Strip trailing slash from path.
    let path_normalized = path_part.trim_end_matches('/');

    // Rebuild the URL, keeping only non-utm query parameters.
    let canon = if let Some(query) = query_part {
        let filtered: Vec<&str> = query
            .split('&')
            .filter(|p| !p.to_lowercase().starts_with("utm_"))
            .collect();
        if filtered.is_empty() {
            path_normalized.to_lowercase()
        } else {
            format!("{}?{}", path_normalized.to_lowercase(), filtered.join("&"))
        }
    } else {
        path_normalized.to_lowercase()
    };

    if canon.is_empty() { None } else { Some(canon) }
}

/// Normalize a title for exact-match deduplication.
///
/// Uses `to_lowercase()` as a practical substitute for Unicode NFKC
/// normalization (the `unicode_normalization` crate is not a workspace dep).
/// The match is exact after normalization — near-identical titles from wire
/// republication are intentionally preserved as distinct articles.
fn canonical_title(title: &str) -> String {
    title.trim().to_lowercase()
}

// ─── News merge ──────────────────────────────────────────────────────────────

/// Merge two [`NewsData`] collections, deduplicating articles and sorting the
/// result newest-first.
///
/// Deduplication strategy:
/// 1. URL-first: if both articles have a canonical URL (after shortener
///    filtering), deduplicate on that.
/// 2. Title-fallback: when at least one side is missing a canonical URL,
///    deduplicate on the exact normalized title.
///
/// `macro_events` are preserved from `primary` (Finnhub); Yahoo's events are
/// always empty so this is a no-op in practice but the field is correct if the
/// source order ever changes.
fn merge_news(primary: NewsData, secondary: NewsData) -> NewsData {
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut seen_titles: HashSet<String> = HashSet::new();
    let mut merged: Vec<NewsArticle> = Vec::new();

    // Helper closure: returns `true` if the article is a duplicate.
    let mut is_duplicate = |article: &NewsArticle| -> bool {
        let url_key = article.url.as_deref().and_then(canonical_url);
        if let Some(ref key) = url_key {
            if !seen_urls.insert(key.clone()) {
                return true;
            }
            // Even if the URL is new, still track the title so a title-only
            // duplicate from the other provider doesn't sneak in.
            seen_titles.insert(canonical_title(&article.title));
            return false;
        }
        // No usable canonical URL — fall back to title dedup.
        let title_key = canonical_title(&article.title);
        !seen_titles.insert(title_key)
    };

    for article in primary.articles {
        if !is_duplicate(&article) {
            merged.push(article);
        }
    }
    for article in secondary.articles {
        if !is_duplicate(&article) {
            merged.push(article);
        }
    }

    // Sort newest-first on RFC3339 strings (lexicographic ordering works for
    // RFC3339 when UTC offsets are consistent, which our normalizers guarantee).
    merged.sort_unstable_by(|a, b| b.published_at.cmp(&a.published_at));
    merged.truncate(NEWS_PREFETCH_MAX_ARTICLES);

    // Use the primary summary as the base, appending a count note.
    let total = merged.len();
    NewsData {
        articles: merged,
        macro_events: primary.macro_events,
        summary: format!("{total} articles from 2 providers"),
    }
}

/// Pre-fetch news from both Finnhub and Yahoo Finance, merge, and deduplicate.
///
/// Returns `Some(Arc<NewsData>)` on success (at least one provider succeeded).
/// Returns `None` only when **both** providers failed — callers fall back to
/// live `GetNews` tool calls in that case.
pub async fn prefetch_analyst_news(
    finnhub_news: &impl NewsProvider,
    yfinance_news: &impl NewsProvider,
    symbol: &str,
) -> Option<Arc<NewsData>> {
    // Resolve the string symbol once to the typed Symbol so both providers use
    // the same canonical form.
    let typed_symbol = match Ticker::parse(symbol) {
        Ok(ticker) => Symbol::Equity(ticker),
        Err(err) => {
            warn!(error = %err, symbol, "news pre-fetch: symbol parse failed");
            return None;
        }
    };

    let (finnhub_result, yahoo_result) = tokio::join!(
        finnhub_news.fetch(&typed_symbol),
        yfinance_news.fetch(&typed_symbol),
    );

    match (finnhub_result, yahoo_result) {
        (Ok(fh), Ok(yf)) => Some(Arc::new(merge_news(fh, yf))),
        (Ok(fh), Err(yf_err)) => {
            warn!(error = %yf_err, symbol, "yahoo news pre-fetch failed; using finnhub only");
            Some(Arc::new(fh))
        }
        (Err(fh_err), Ok(yf)) => {
            warn!(error = %fh_err, symbol, "finnhub news pre-fetch failed; using yahoo only");
            Some(Arc::new(yf))
        }
        (Err(fh_err), Err(yf_err)) => {
            warn!(
                finnhub_error = %fh_err,
                yahoo_error = %yf_err,
                symbol,
                "both news pre-fetches failed; analysts will use live tool calls"
            );
            None
        }
    }
}

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
    fred: &FredClient,
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
    let analyst_handles = state.analyst_handles();
    let model_id = handle.model_id().to_owned();

    // Resolve the active pack's runtime policy once. PreflightTask is the
    // sole writer of `state.analysis_runtime_policy`; if it has not run
    // before this orchestrator (only possible from tests that bypass
    // preflight), produce a typed error rather than silently rendering
    // against missing context.
    let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!(
            "run_analyst_team: missing runtime policy — preflight is the sole writer of \
             state.analysis_runtime_policy; tests bypassing preflight must use \
             `with_baseline_runtime_policy`"
        ))
    })?;

    // ── Pre-fetch news once; both Sentiment and News analysts share the result ─
    //
    // This eliminates the duplicate Finnhub `get_news` call (P1).  If the
    // pre-fetch fails the analysts fall back to their live `GetNews` tool.
    let yfinance_news_provider = YFinanceNewsProvider::new(yfinance);
    let cached_news = prefetch_analyst_news(finnhub, &yfinance_news_provider, &symbol).await;

    // ── Spawn all four analysts concurrently ─────────────────────────────

    let fundamental_task = {
        let analyst =
            FundamentalAnalyst::new(handle.clone(), finnhub.clone(), state, policy, llm_config);
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let sentiment_task = {
        let analyst = SentimentAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            state,
            policy,
            llm_config,
            cached_news.clone(),
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let news_task = {
        let analyst = NewsAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            fred.clone(),
            state,
            policy,
            llm_config,
            cached_news,
        );
        tokio::spawn(async move { tokio::time::timeout(outer_timeout, analyst.run()).await })
    };

    let technical_task = {
        let analyst =
            TechnicalAnalyst::new(handle.clone(), yfinance.clone(), state, policy, llm_config);
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
                url: None,
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
            options_summary: None,
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
            rate_limit_wait_ms: 0,
        }
    }

    // ── flatten_task_result ──────────────────────────────────────────────

    // TC-20: rename from the misleading "flatten_join_error_becomes_analyst_error"
    // — this test exercises the *success* path, not a join error.
    #[test]
    fn flatten_task_result_success_path() {
        let ok: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Ok(Ok(42)));
        let result = flatten_task_result::<i32>("test", ok);
        assert_eq!(result.unwrap(), 42);
    }

    // TC-1: timeout branch → NetworkTimeout
    #[tokio::test]
    async fn flatten_task_result_timeout_becomes_network_timeout() {
        use std::future::pending;
        // Drive a zero-duration timeout to obtain a real Elapsed value.
        let elapsed = tokio::time::timeout(Duration::ZERO, pending::<()>())
            .await
            .unwrap_err();
        let timeout_result: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Err(elapsed));
        let result = flatten_task_result::<i32>("Fundamental Analyst", timeout_result);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TradingError::NetworkTimeout { .. }),
            "timed-out task must map to NetworkTimeout"
        );
    }

    // TC-2: JoinError branch → AnalystError
    #[tokio::test]
    async fn flatten_task_result_join_error_becomes_analyst_error() {
        // A task that panics produces a JoinError when awaited.
        let join_err = tokio::spawn(async { panic!("deliberate test panic") })
            .await
            .unwrap_err();
        let join_result: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Err(join_err);
        let result = flatten_task_result::<i32>("Sentiment Analyst", join_result);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), TradingError::AnalystError { .. }),
            "panicked task must map to AnalystError"
        );
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
            valuation_fetch_timeout_secs: 30,
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
        assert!(state.fundamental_metrics().is_some());
        assert!(state.market_sentiment().is_some());
        assert!(state.macro_news().is_some());
        assert!(state.technical_indicators().is_some());
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
        assert!(state.fundamental_metrics().is_some());
        assert!(
            state.market_sentiment().is_none(),
            "failed analyst field must be None"
        );
        assert!(state.macro_news().is_some());
        assert!(state.technical_indicators().is_some());
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

    // ── Task 6.2 (extended): three and four failures also abort ─────────

    #[tokio::test]
    async fn three_failures_abort() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        let result = apply_analyst_results(
            Err(TradingError::Rig("err1".to_owned())),
            Err(TradingError::Rig("err2".to_owned())),
            Err(TradingError::Rig("err3".to_owned())),
            Ok((sample_technical(), sample_usage("Technical Analyst"))),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TradingError::AnalystError { .. }),
            "three failures must abort with AnalystError"
        );
    }

    #[tokio::test]
    async fn four_failures_abort() {
        let mut state = TradingState::new("AAPL", "2026-03-14");
        let handles = state.analyst_handles();

        let result = apply_analyst_results(
            Err(TradingError::Rig("err1".to_owned())),
            Err(TradingError::Rig("err2".to_owned())),
            Err(TradingError::Rig("err3".to_owned())),
            Err(TradingError::Rig("err4".to_owned())),
            &handles,
            &mut state,
            "gpt-4o-mini",
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TradingError::AnalystError { .. }),
            "four failures must abort with AnalystError"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Merge / dedupe tests (Task 5)
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod merge_tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{
        domain::Symbol,
        error::TradingError,
        state::{ImpactDirection, MacroEvent, NewsArticle, NewsData},
    };

    // ── Stub NewsProvider ────────────────────────────────────────────────

    struct StubNewsProvider {
        data: Option<NewsData>,
        should_fail: bool,
    }

    impl StubNewsProvider {
        fn ok(data: NewsData) -> Self {
            Self {
                data: Some(data),
                should_fail: false,
            }
        }

        fn err() -> Self {
            Self {
                data: None,
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl NewsProvider for StubNewsProvider {
        fn provider_name(&self) -> &'static str {
            "stub"
        }

        async fn fetch(&self, _symbol: &Symbol) -> Result<NewsData, TradingError> {
            if self.should_fail {
                Err(TradingError::NetworkTimeout {
                    elapsed: Duration::ZERO,
                    message: "stubbed failure".to_owned(),
                })
            } else {
                Ok(self
                    .data
                    .clone()
                    .expect("StubNewsProvider::ok must have data"))
            }
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn article(title: &str, url: Option<&str>, published_at: &str) -> NewsArticle {
        NewsArticle {
            title: title.to_owned(),
            source: "TestSource".to_owned(),
            published_at: published_at.to_owned(),
            relevance_score: None,
            snippet: String::new(),
            url: url.map(str::to_owned),
        }
    }

    fn news_data(articles: Vec<NewsArticle>) -> NewsData {
        NewsData {
            articles,
            macro_events: vec![],
            summary: "test".to_owned(),
        }
    }

    fn news_data_with_events(articles: Vec<NewsArticle>, events: Vec<MacroEvent>) -> NewsData {
        NewsData {
            articles,
            macro_events: events,
            summary: "test".to_owned(),
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn merge_dedupes_by_url() {
        // Finnhub and Yahoo return the same article identified by the same URL.
        let shared_url = "https://example.com/article/aapl-q4";
        let fh = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some(shared_url),
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some(shared_url),
            "2026-03-14T10:00:00Z",
        )]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed when both providers succeed");

        assert_eq!(
            result.articles.len(),
            1,
            "duplicate URL must be deduped to a single article"
        );
    }

    #[tokio::test]
    async fn merge_dedupes_by_headline_when_url_missing() {
        // Both providers return the same article but with no URL; dedup on title.
        let fh = news_data(vec![article(
            "Apple Posts Strong Q4",
            None,
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Apple Posts Strong Q4",
            None,
            "2026-03-14T10:00:00Z",
        )]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed");

        assert_eq!(
            result.articles.len(),
            1,
            "duplicate title (no URL) must be deduped to a single article"
        );
    }

    #[tokio::test]
    async fn merge_dedupes_same_article_when_canonical_url_differs_via_redirect_resolution() {
        // Finnhub stores the canonical publisher URL.
        // Yahoo stores a yhoo.it shortener for the same article.
        // The shortener is treated as "no URL" → falls back to title-hash dedup.
        let canonical = "https://reuters.com/technology/apple-q4-2026";
        let shortener = "https://yhoo.it/abc123";

        let fh = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some(canonical),
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some(shortener),
            "2026-03-14T10:00:00Z",
        )]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed");

        // The yhoo.it shortener must be treated as missing URL, so dedup falls
        // back to title-hash. Same title => single article.
        assert_eq!(
            result.articles.len(),
            1,
            "same article via shortener+canonical must be deduped; analyst must not see fake two-source signal"
        );
    }

    #[tokio::test]
    async fn merge_preserves_multi_outlet_coverage_for_wire_republication() {
        // Same AP wire copy republished by 5 outlets. Each has a distinct URL
        // and a slightly different title (NOT byte-identical). The merge must
        // NOT collapse these — broad coverage is itself a signal.
        let fh = news_data(vec![
            article(
                "Apple Q4 Results Beat Estimates",
                Some("https://reuters.com/aapl-q4"),
                "2026-03-14T10:00:00Z",
            ),
            article(
                "Apple Fourth Quarter Results Beat Wall Street Estimates",
                Some("https://bloomberg.com/aapl-q4"),
                "2026-03-14T10:01:00Z",
            ),
            article(
                "Apple Reports Q4 Earnings Beat",
                Some("https://cnbc.com/aapl-q4"),
                "2026-03-14T10:02:00Z",
            ),
        ]);
        let yf = news_data(vec![
            article(
                "Apple Q4 Profit Exceeds Analyst Forecasts",
                Some("https://wsj.com/aapl-q4"),
                "2026-03-14T10:03:00Z",
            ),
            article(
                "Apple Beats Q4 Earnings Expectations",
                Some("https://ft.com/aapl-q4"),
                "2026-03-14T10:04:00Z",
            ),
        ]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed");

        // All 5 articles have distinct URLs and near-but-not-identical titles.
        // Title-hash dedup uses exact match after lowercasing, so none of these
        // should be collapsed.
        assert_eq!(
            result.articles.len(),
            5,
            "5 distinct republications must all survive — broad coverage is decision-relevant"
        );
    }

    #[tokio::test]
    async fn merge_falls_back_to_single_provider_on_partial_failure() {
        let fh = news_data(vec![article(
            "Apple Q4 Beat",
            Some("https://reuters.com/aapl"),
            "2026-03-14T10:00:00Z",
        )]);

        // Yahoo fails.
        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::err(), "AAPL")
                .await
                .expect("should succeed with Finnhub-only fallback when Yahoo fails");

        assert_eq!(
            result.articles.len(),
            1,
            "single provider fallback must return available articles"
        );
    }

    #[tokio::test]
    async fn merge_falls_back_to_yahoo_only_when_finnhub_fails() {
        let yf = news_data(vec![article(
            "Apple Q4 Beat",
            Some("https://finance.yahoo.com/aapl"),
            "2026-03-14T10:00:00Z",
        )]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::err(), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed with Yahoo-only fallback when Finnhub fails");

        assert_eq!(
            result.articles.len(),
            1,
            "single provider fallback must return available articles"
        );
    }

    #[tokio::test]
    async fn prefetch_analyst_news_returns_none_when_both_prefetch_providers_fail() {
        let result =
            prefetch_analyst_news(&StubNewsProvider::err(), &StubNewsProvider::err(), "AAPL").await;

        assert!(
            result.is_none(),
            "must return None when both providers fail so live tool fallback activates"
        );
    }

    #[tokio::test]
    async fn merge_sorts_articles_newest_first() {
        let fh = news_data(vec![
            article("Oldest Article", None, "2026-03-14T08:00:00Z"),
            article("Middle Article", None, "2026-03-14T10:00:00Z"),
        ]);
        let yf = news_data(vec![article(
            "Newest Article",
            None,
            "2026-03-14T12:00:00Z",
        )]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed");

        assert_eq!(result.articles.len(), 3);
        // Verify newest-first ordering.
        assert_eq!(
            result.articles[0].title, "Newest Article",
            "first article should be the newest"
        );
        assert_eq!(
            result.articles[1].title, "Middle Article",
            "second article should be the middle one"
        );
        assert_eq!(
            result.articles[2].title, "Oldest Article",
            "last article should be the oldest"
        );
    }

    // ── canonical_url unit tests ─────────────────────────────────────────

    #[test]
    fn canonical_url_strips_utm_params() {
        let url = "https://example.com/article?utm_source=twitter&utm_medium=social&real=1";
        let canon = canonical_url(url).expect("should canonicalize");
        assert!(
            !canon.contains("utm_"),
            "UTM params must be stripped; got: {canon}"
        );
        assert!(
            canon.contains("real=1"),
            "non-utm params must be preserved; got: {canon}"
        );
    }

    #[test]
    fn canonical_url_rejects_shortener_host() {
        assert!(
            canonical_url("https://yhoo.it/abc123").is_none(),
            "yhoo.it shortener must return None"
        );
        assert!(
            canonical_url("https://bit.ly/abc").is_none(),
            "bit.ly shortener must return None"
        );
    }

    #[test]
    fn canonical_url_strips_trailing_slash() {
        let with_slash = canonical_url("https://example.com/article/").unwrap();
        let without_slash = canonical_url("https://example.com/article").unwrap();
        assert_eq!(
            with_slash, without_slash,
            "trailing slash must be normalized away"
        );
    }

    #[test]
    fn canonical_url_lowercases() {
        let mixed = canonical_url("https://Example.COM/Article").unwrap();
        let lower = canonical_url("https://example.com/article").unwrap();
        assert_eq!(mixed, lower, "URL must be lowercased for comparison");
    }

    // ── macro_events preservation ────────────────────────────────────────

    #[tokio::test]
    async fn merge_preserves_finnhub_macro_events() {
        let event = MacroEvent {
            event: "Fed rate cut".to_owned(),
            impact_direction: ImpactDirection::Positive,
            confidence: 0.8,
        };
        let fh = news_data_with_events(
            vec![article("Article A", None, "2026-03-14T10:00:00Z")],
            vec![event.clone()],
        );
        let yf = news_data(vec![article("Article B", None, "2026-03-14T09:00:00Z")]);

        let result =
            prefetch_analyst_news(&StubNewsProvider::ok(fh), &StubNewsProvider::ok(yf), "AAPL")
                .await
                .expect("should succeed");

        assert_eq!(
            result.macro_events.len(),
            1,
            "finnhub macro events must be preserved"
        );
        assert_eq!(result.macro_events[0].event, "Fed rate cut");
    }
}
