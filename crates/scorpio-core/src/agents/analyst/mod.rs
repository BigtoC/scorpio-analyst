//! Analyst team: specialist agents that produce structured data for the
//! downstream debate and trading pipeline.
//!
//! # Fan-out execution
//!
//! The equity analyst set runs concurrently via the registry-driven task
//! fan-out in `workflow::pipeline`. The degradation policy tolerates one
//! failure (partial data continues); two or more failures abort the cycle.
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

use tracing::warn;

use crate::{
    data::traits::NewsProvider,
    domain::{Symbol, Ticker},
    state::{NewsArticle, NewsData},
};

#[cfg(test)]
use crate::{config::LlmConfig, error::RetryPolicy};
#[cfg(test)]
use std::time::Duration;

/// Maximum number of articles kept after merging Finnhub and Yahoo news.
const NEWS_PREFETCH_MAX_ARTICLES: usize = 30;

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
/// Returns `None` when the URL belongs to a known shortener with no embedded
/// target URL (so callers fall back to title-hash dedup) or when the input is
/// empty/whitespace. Otherwise, returns a normalized form: scheme+host+path in
/// lowercase, trailing slashes stripped, and `?utm_*` query parameters removed.
fn canonical_url(url: &str) -> Option<String> {
    fn percent_decode(input: &str) -> Option<String> {
        fn hex_value(byte: u8) -> Option<u8> {
            match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            }
        }

        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'%' if i + 2 < bytes.len() => {
                    let hi = hex_value(bytes[i + 1])?;
                    let lo = hex_value(bytes[i + 2])?;
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                byte => {
                    out.push(byte);
                    i += 1;
                }
            }
        }

        String::from_utf8(out).ok()
    }

    fn embedded_shortener_target(query: &str) -> Option<String> {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            if !matches!(key.to_ascii_lowercase().as_str(), "url" | "u" | "target") {
                continue;
            }
            let decoded = percent_decode(value)?;
            if decoded.trim().is_empty() {
                continue;
            }
            return canonical_url(&decoded);
        }
        None
    }

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

    // Split off query string.
    let (path_part, query_part) = match without_scheme.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (without_scheme, None),
    };

    // Extract just the host (up to the first `/`).
    let host_end = path_part.find('/').unwrap_or(path_part.len());
    let host = &path_part[..host_end];

    // Deterministically canonicalize known shorteners when the wrapped target
    // URL is available in the query string; otherwise fall back to title dedupe.
    let host_lower = host.to_ascii_lowercase();
    if SHORTENER_HOSTS.iter().any(|&s| host_lower == s) {
        return query_part.and_then(embedded_shortener_target);
    }

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

fn sort_and_cap_news(mut news: NewsData) -> NewsData {
    news.articles
        .sort_unstable_by(|a, b| b.published_at.cmp(&a.published_at));
    news.articles.truncate(NEWS_PREFETCH_MAX_ARTICLES);
    news
}

fn build_sentiment_news(vetted: &NewsData, reddit: NewsData) -> Option<NewsData> {
    if reddit.articles.is_empty() {
        return if vetted.articles.is_empty() {
            None
        } else {
            Some(vetted.clone())
        };
    }

    let mut vetted_articles = vetted.articles.clone();
    vetted_articles.sort_unstable_by(|a, b| b.published_at.cmp(&a.published_at));

    let reddit_count = reddit.articles.len();
    let mut articles = vetted_articles;
    articles.extend(reddit.articles);
    articles.sort_unstable_by(|a, b| b.published_at.cmp(&a.published_at));

    if articles.is_empty() {
        return None;
    }

    let kept_reddit = articles
        .iter()
        .filter(|a| a.source.starts_with("Reddit r/"))
        .count();
    let kept_vetted = articles.len().saturating_sub(kept_reddit);

    Some(NewsData {
        articles,
        macro_events: vetted.macro_events.clone(),
        summary: format!(
            "{kept_vetted} vetted articles + {kept_reddit} Reddit articles (of {reddit_count} fetched)"
        ),
    })
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
    let mut seen_titles_without_url: HashSet<String> = HashSet::new();
    let mut merged: Vec<NewsArticle> = Vec::new();

    // Helper closure: returns `true` if the article is a duplicate.
    let mut is_duplicate = |article: &NewsArticle| -> bool {
        let title_key = canonical_title(&article.title);
        let url_key = article.url.as_deref().and_then(canonical_url);
        if let Some(ref key) = url_key {
            if seen_titles_without_url.contains(&title_key) {
                return true;
            }
            if !seen_urls.insert(key.clone()) {
                return true;
            }
            // Even if the URL is new, still track the title so a title-only
            // duplicate from the other provider doesn't sneak in.
            seen_titles.insert(title_key);
            return false;
        }
        // No usable canonical URL — fall back to title dedup.
        if !seen_titles.insert(title_key.clone()) {
            return true;
        }
        seen_titles_without_url.insert(title_key);
        false
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

/// Dual-feed result of [`prefetch_analyst_news`].
///
/// - `vetted` is `Some` when at least one of Finnhub or Yahoo succeeded.
///   Bound into `NewsAnalyst` via `KEY_CACHED_VETTED_NEWS`.
/// - `sentiment` is `Some` when at least one of the three providers
///   succeeded (vetted + Reddit sidecar). Bound into `SentimentAnalyst`
///   via `KEY_CACHED_SENTIMENT_NEWS`.
#[derive(Debug, Clone, Default)]
pub struct PrefetchedNewsBundle {
    pub vetted: Option<Arc<NewsData>>,
    pub sentiment: Option<Arc<NewsData>>,
}

/// Pre-fetch news from Finnhub, Yahoo, and Reddit and build two analyst feeds.
///
/// - Vetted feed (Finnhub + Yahoo, deduplicated and sorted) → `NewsAnalyst`.
/// - Sentiment feed (vetted + Reddit sidecar, preserving Reddit rows as
///   distinct sentiment inputs) → `SentimentAnalyst`.
///
/// Reddit never displaces vetted rows from `NewsAnalyst`. The sentiment lane
/// reports `None` when every contributing provider either failed or produced
/// no usable articles after filtering, so callers still fall back to live
/// `GetNews` when cached sentiment content is empty.
pub async fn prefetch_analyst_news(
    finnhub_news: &impl NewsProvider,
    yfinance_news: &impl NewsProvider,
    reddit_news: &impl NewsProvider,
    symbol: &str,
) -> PrefetchedNewsBundle {
    let typed_symbol = match Ticker::parse(symbol) {
        Ok(ticker) => Symbol::Equity(ticker),
        Err(err) => {
            warn!(error = %err, symbol, "news pre-fetch: symbol parse failed");
            return PrefetchedNewsBundle::default();
        }
    };

    let (finnhub_result, yahoo_result, reddit_result) = tokio::join!(
        finnhub_news.fetch(&typed_symbol),
        yfinance_news.fetch(&typed_symbol),
        reddit_news.fetch(&typed_symbol),
    );

    let vetted: Option<NewsData> = match (finnhub_result, yahoo_result) {
        (Ok(fh), Ok(yf)) => Some(merge_news(fh, yf)),
        (Ok(fh), Err(yf_err)) => {
            warn!(error = %yf_err, symbol, "yahoo news pre-fetch failed; using finnhub only");
            Some(sort_and_cap_news(fh))
        }
        (Err(fh_err), Ok(yf)) => {
            warn!(error = %fh_err, symbol, "finnhub news pre-fetch failed; using yahoo only");
            Some(sort_and_cap_news(yf))
        }
        (Err(fh_err), Err(yf_err)) => {
            warn!(
                finnhub_error = %fh_err,
                yahoo_error = %yf_err,
                symbol,
                "both vetted news pre-fetches failed; NewsAnalyst will fall back to live GetNews"
            );
            None
        }
    };

    let reddit_news_data: Option<NewsData> = match reddit_result {
        Ok(data) => Some(data),
        Err(err) => {
            warn!(
                reddit_error = %err,
                symbol,
                "reddit news pre-fetch failed; sentiment lane continues without sidecar"
            );
            None
        }
    };

    let sentiment: Option<NewsData> = match (vetted.as_ref(), reddit_news_data) {
        (None, None) => None,
        (Some(v), None) => {
            if v.articles.is_empty() {
                None
            } else {
                Some(v.clone())
            }
        }
        (None, Some(r)) => {
            let reddit_only = sort_and_cap_news(r);
            if reddit_only.articles.is_empty() {
                None
            } else {
                Some(reddit_only)
            }
        }
        (Some(v), Some(r)) => build_sentiment_news(v, r),
    };

    PrefetchedNewsBundle {
        vetted: vetted.map(Arc::new),
        sentiment: sentiment.map(Arc::new),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Helper that builds an empty-news Reddit stub. The Reddit sidecar
    /// is the third positional argument to `prefetch_analyst_news` after the
    /// lane-split refactor; tests that don't care about the sidecar use this.
    fn empty_reddit_stub() -> StubNewsProvider {
        StubNewsProvider::ok(NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: String::new(),
        })
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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle
            .vetted
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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

        // The yhoo.it shortener must be treated as missing URL, so dedup falls
        // back to title-hash. Same title => single article.
        assert_eq!(
            result.articles.len(),
            1,
            "same article via shortener+canonical must be deduped; analyst must not see fake two-source signal"
        );
    }

    #[tokio::test]
    async fn merge_dedupes_shortener_target_url_even_when_titles_do_not_exactly_match() {
        let canonical = "https://reuters.com/technology/apple-q4-2026";
        let wrapped = "https://yhoo.it/abc123?url=https%3A%2F%2Freuters.com%2Ftechnology%2Fapple-q4-2026&utm_source=yahoo";

        let fh = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some(canonical),
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Apple Posts Strong Q4 on Reuters",
            Some(wrapped),
            "2026-03-14T10:00:00Z",
        )]);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

        assert_eq!(
            result.articles.len(),
            1,
            "known shortener URLs with an embedded canonical target must dedupe by canonical URL even when the titles are not an exact match"
        );
    }

    #[tokio::test]
    async fn merge_dedupes_by_title_when_only_one_provider_has_canonical_url() {
        let fh = news_data(vec![article(
            "Apple Posts Strong Q4",
            None,
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Apple Posts Strong Q4",
            Some("https://reuters.com/technology/apple-q4-2026"),
            "2026-03-14T10:00:00Z",
        )]);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

        assert_eq!(
            result.articles.len(),
            1,
            "when either side lacks a canonical URL, merge must fall back to title dedupe"
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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

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
        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::err(),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle
            .vetted
            .expect("should succeed with Finnhub-only fallback when Yahoo fails");

        assert_eq!(
            result.articles.len(),
            1,
            "single provider fallback must return available articles"
        );
    }

    #[tokio::test]
    async fn single_provider_fallback_applies_article_cap() {
        let articles: Vec<NewsArticle> = (0..(NEWS_PREFETCH_MAX_ARTICLES + 5))
            .map(|idx| {
                article(
                    &format!("Article {idx}"),
                    Some(&format!("https://reuters.com/article-{idx}")),
                    &format!("2026-03-14T10:00:{idx:02}Z"),
                )
            })
            .collect();
        let fh = news_data(articles);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::err(),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle
            .vetted
            .expect("single-provider fallback should still succeed");

        assert_eq!(
            result.articles.len(),
            NEWS_PREFETCH_MAX_ARTICLES,
            "single-provider fallback must still respect the cached-news article cap"
        );
    }

    #[tokio::test]
    async fn merge_falls_back_to_yahoo_only_when_finnhub_fails() {
        let yf = news_data(vec![article(
            "Apple Q4 Beat",
            Some("https://finance.yahoo.com/aapl"),
            "2026-03-14T10:00:00Z",
        )]);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::err(),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle
            .vetted
            .expect("should succeed with Yahoo-only fallback when Finnhub fails");

        assert_eq!(
            result.articles.len(),
            1,
            "single provider fallback must return available articles"
        );
    }

    #[tokio::test]
    async fn prefetch_all_three_fail_returns_none_for_both_lanes() {
        let bundle = prefetch_analyst_news(
            &StubNewsProvider::err(),
            &StubNewsProvider::err(),
            &StubNewsProvider::err(),
            "AAPL",
        )
        .await;

        assert!(
            bundle.vetted.is_none(),
            "must return None for the vetted lane when both vetted providers fail"
        );
        assert!(
            bundle.sentiment.is_none(),
            "must return None for the sentiment lane when every contributor fails"
        );
    }

    #[tokio::test]
    async fn prefetch_returns_sentiment_only_when_finnhub_and_yahoo_fail_but_reddit_succeeds() {
        let mut reddit_article = article(
            "AAPL Q4 thread",
            Some("https://www.reddit.com/r/stocks/comments/abc/aapl_q4/"),
            "2026-03-14T10:00:00Z",
        );
        reddit_article.source = "Reddit r/stocks".to_owned();
        let reddit = news_data(vec![reddit_article]);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::err(),
            &StubNewsProvider::err(),
            &StubNewsProvider::ok(reddit),
            "AAPL",
        )
        .await;

        assert!(bundle.vetted.is_none(), "no vetted source succeeded");
        let sentiment = bundle
            .sentiment
            .expect("sentiment feed should exist when only Reddit succeeded");
        assert_eq!(sentiment.articles.len(), 1);
        assert!(sentiment.articles[0].source.starts_with("Reddit r/"));
    }

    #[tokio::test]
    async fn prefetch_vetted_never_contains_reddit_sources() {
        let fh = news_data(vec![article(
            "Vetted Article",
            Some("https://reuters.com/aapl"),
            "2026-03-14T10:00:00Z",
        )]);
        let yf = news_data(vec![article(
            "Yahoo Article",
            Some("https://finance.yahoo.com/aapl"),
            "2026-03-14T09:00:00Z",
        )]);
        let mut reddit_article = article(
            "Reddit Article",
            Some("https://www.reddit.com/r/stocks/comments/abc/x/"),
            "2026-03-14T08:00:00Z",
        );
        reddit_article.source = "Reddit r/stocks".to_owned();
        let reddit = news_data(vec![reddit_article]);

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &StubNewsProvider::ok(reddit),
            "AAPL",
        )
        .await;

        let vetted = bundle.vetted.expect("vetted feed should exist");
        assert!(
            vetted
                .articles
                .iter()
                .all(|a| !a.source.starts_with("Reddit r/")),
            "vetted feed must never contain Reddit sources"
        );

        let sentiment = bundle.sentiment.expect("sentiment feed should exist");
        let reddit_count = sentiment
            .articles
            .iter()
            .filter(|a| a.source.starts_with("Reddit r/"))
            .count();
        assert!(reddit_count >= 1, "sentiment feed must carry Reddit rows");
    }

    #[tokio::test]
    async fn prefetch_empty_reddit_ok_does_not_create_sentiment_when_vetted_fails() {
        // Reddit returns Ok with empty articles. With both vetted providers
        // failing, the sentiment lane should remain None so callers fall back
        // to live GetNews instead of pinning a useless empty feed.
        let bundle = prefetch_analyst_news(
            &StubNewsProvider::err(),
            &StubNewsProvider::err(),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;

        assert!(bundle.vetted.is_none());
        assert!(
            bundle.sentiment.is_none(),
            "empty Reddit Ok must not suppress fallback path"
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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

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
    fn canonical_url_extracts_nested_target_from_known_shortener_query() {
        let wrapped = "https://yhoo.it/abc123?url=https%3A%2F%2Freuters.com%2Ftechnology%2Fapple-q4-2026&utm_source=yahoo";
        let canon = canonical_url(wrapped)
            .expect("shortener with embedded target URL should canonicalize deterministically");

        assert_eq!(
            canon, "reuters.com/technology/apple-q4-2026",
            "known shorteners should canonicalize to the embedded target URL when it is available"
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

        let bundle = prefetch_analyst_news(
            &StubNewsProvider::ok(fh),
            &StubNewsProvider::ok(yf),
            &empty_reddit_stub(),
            "AAPL",
        )
        .await;
        let result = bundle.vetted.expect("vetted lane should exist");

        assert_eq!(
            result.macro_events.len(),
            1,
            "finnhub macro events must be preserved"
        );
        assert_eq!(result.macro_events[0].event, "Fed rate cut");
    }
}
