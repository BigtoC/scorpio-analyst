//! [`RedditNewsProvider`] — sentiment-sidecar [`NewsProvider`].
//!
//! Output is bound only into [`crate::agents::analyst::equity::SentimentAnalyst`]
//! via [`crate::agents::analyst::prefetch_analyst_news`]. The vetted
//! [`crate::agents::analyst::equity::NewsAnalyst`] lane never consumes Reddit data.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::{
    constants::{
        NEWS_ANALYSIS_DAYS, NEWS_SNIPPET_MAX_CHARS, NEWS_TITLE_MAX_CHARS,
        REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST, REDDIT_MIN_SCORE, REDDIT_PER_SUB_FETCH_LIMIT,
        REDDIT_SENTIMENT_MAX_ARTICLES,
    },
    data::traits::NewsProvider,
    domain::Symbol,
    error::TradingError,
    state::{NewsArticle, NewsData},
};

use super::{client::RedditClient, types::RawSubmission};

/// Sentiment-sidecar Reddit news provider.
#[derive(Clone, Debug)]
pub struct RedditNewsProvider {
    client: RedditClient,
    subreddits: Vec<String>,
}

impl RedditNewsProvider {
    /// Construct a Reddit news provider for a fixed set of subreddits.
    #[must_use]
    pub fn new(client: RedditClient, subreddits: Vec<String>) -> Self {
        Self { client, subreddits }
    }

    /// Extract the canonical equity ticker for Reddit search.
    ///
    /// Non-equity symbols are out of scope in v1 and return `None`, so the
    /// caller short-circuits with empty `NewsData`.
    fn ticker_for_search(symbol: &Symbol) -> Option<String> {
        symbol.as_equity().map(|t| t.as_str().to_owned())
    }
}

/// True when `ticker` should be skipped to avoid unrelated Reddit matches.
fn is_ambiguous(ticker: &str) -> bool {
    REDDIT_AMBIGUOUS_SYMBOLS_DENYLIST
        .iter()
        .any(|deny| deny.eq_ignore_ascii_case(ticker))
}

/// Drop submissions that fail any defense-in-depth filter.
fn keep_submission(post: &RawSubmission, cutoff: DateTime<Utc>) -> bool {
    if post.over_18 || post.stickied {
        return false;
    }
    // RSS-sourced posts have no score; skip the score floor for them.
    if !post.via_rss && post.score < i64::from(REDDIT_MIN_SCORE) {
        return false;
    }
    let Some(created) = DateTime::<Utc>::from_timestamp(post.created_utc as i64, 0) else {
        return false;
    };
    created >= cutoff
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

fn compute_relevance_score(post: &RawSubmission) -> Option<f64> {
    // Score is unavailable for RSS-sourced posts; omit rather than fake zeros.
    if post.via_rss {
        return None;
    }
    let s = post.score.max(0) as f64;
    let raw = ((s + 1.0).log10()) / (1000_f64.log10());
    Some(raw.clamp(0.0, 1.0))
}

/// Convert a retained `RawSubmission` to a `NewsArticle`.
fn normalize(post: &RawSubmission) -> NewsArticle {
    let published_at = DateTime::<Utc>::from_timestamp(post.created_utc as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| post.created_utc.to_string());
    // Mark RSS-sourced posts so consumers know scores/counts are unavailable.
    let source = if post.via_rss {
        format!("Reddit r/{} (RSS, scores unavailable)", post.subreddit)
    } else {
        format!("Reddit r/{}", post.subreddit)
    };
    NewsArticle {
        title: truncate_chars(&post.title, NEWS_TITLE_MAX_CHARS),
        source,
        published_at,
        relevance_score: compute_relevance_score(post),
        snippet: truncate_chars(&post.selftext, NEWS_SNIPPET_MAX_CHARS),
        url: Some(format!("https://www.reddit.com{}", post.permalink)),
    }
}

fn empty_news_data(reason: &str) -> NewsData {
    NewsData {
        articles: vec![],
        macro_events: vec![],
        summary: format!("Reddit: 0 posts ({reason})"),
    }
}

#[async_trait]
impl NewsProvider for RedditNewsProvider {
    fn provider_name(&self) -> &'static str {
        "reddit"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        let Some(ticker) = Self::ticker_for_search(symbol) else {
            return Ok(empty_news_data("unsupported symbol shape"));
        };
        if is_ambiguous(&ticker) {
            return Ok(empty_news_data("ambiguous symbol denylist"));
        }
        if self.subreddits.is_empty() {
            return Ok(empty_news_data("pack opted out or rollout gated"));
        }

        let raw = self
            .client
            .search_submissions(&self.subreddits, &ticker, REDDIT_PER_SUB_FETCH_LIMIT)
            .await?;

        let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;
        let mut retained: Vec<NewsArticle> = raw
            .iter()
            .filter(|post| keep_submission(post, cutoff))
            .map(normalize)
            .collect();

        // Sort: relevance_score desc (monotonic in score), tie-break published_at desc.
        retained.sort_by(|a, b| {
            let ra = a.relevance_score.unwrap_or(0.0);
            let rb = b.relevance_score.unwrap_or(0.0);
            rb.partial_cmp(&ra)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.published_at.cmp(&a.published_at))
        });
        retained.truncate(REDDIT_SENTIMENT_MAX_ARTICLES);

        let count = retained.len();
        let sub_count = self.subreddits.len();
        Ok(NewsData {
            articles: retained,
            macro_events: vec![],
            summary: format!("Reddit: {count} posts from {sub_count} subreddits"),
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Symbol, Ticker};

    fn submission(
        title: &str,
        score: i64,
        created_utc: f64,
        over_18: bool,
        stickied: bool,
    ) -> RawSubmission {
        RawSubmission {
            title: title.to_owned(),
            selftext: "body".to_owned(),
            permalink: "/r/stocks/comments/abc/xyz/".to_owned(),
            subreddit: "stocks".to_owned(),
            created_utc,
            score,
            over_18,
            stickied,
            via_rss: false,
        }
    }

    fn recent_unix() -> f64 {
        Utc::now().timestamp() as f64
    }

    // ── pure-function tests ──────────────────────────────────────────────

    #[test]
    fn denylist_lookup_is_case_insensitive() {
        assert!(is_ambiguous("ALL"));
        assert!(is_ambiguous("all"));
        assert!(is_ambiguous("aLl"));
        assert!(!is_ambiguous("AAPL"));
    }

    #[test]
    fn keep_submission_rejects_nsfw() {
        let p = submission("nsfw", 100, recent_unix(), true, false);
        assert!(!keep_submission(&p, Utc::now() - NEWS_ANALYSIS_DAYS));
    }

    #[test]
    fn keep_submission_rejects_stickied() {
        let p = submission("stickied", 100, recent_unix(), false, true);
        assert!(!keep_submission(&p, Utc::now() - NEWS_ANALYSIS_DAYS));
    }

    #[test]
    fn keep_submission_score_floor_boundary() {
        let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;
        let p_below = submission("49 score", 49, recent_unix(), false, false);
        let p_at = submission("50 score", 50, recent_unix(), false, false);
        assert!(!keep_submission(&p_below, cutoff));
        assert!(keep_submission(&p_at, cutoff));
    }

    #[test]
    fn keep_submission_age_filter() {
        let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;
        let old_unix = (Utc::now() - chrono::Duration::days(60)).timestamp() as f64;
        let p_old = submission("old", 100, old_unix, false, false);
        assert!(!keep_submission(&p_old, cutoff));
        let recent = (Utc::now() - chrono::Duration::days(5)).timestamp() as f64;
        let p_recent = submission("recent", 100, recent, false, false);
        assert!(keep_submission(&p_recent, cutoff));
    }

    #[test]
    fn compute_relevance_score_zero_score_is_zero() {
        let p = submission("t", 0, recent_unix(), false, false);
        let r = compute_relevance_score(&p).unwrap();
        assert!((r - 0.0).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn compute_relevance_score_thousand_is_one() {
        let p = submission("t", 1000, recent_unix(), false, false);
        let r = compute_relevance_score(&p).unwrap();
        assert!((r - 1.0).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn compute_relevance_score_clamps_above_thousand() {
        let p = submission("t", 100_000, recent_unix(), false, false);
        assert_eq!(compute_relevance_score(&p).unwrap(), 1.0);
    }

    #[test]
    fn compute_relevance_score_is_none_for_rss_posts() {
        let mut p = submission("t", 100, recent_unix(), false, false);
        p.via_rss = true;
        assert!(compute_relevance_score(&p).is_none());
    }

    #[test]
    fn normalize_formats_source_as_reddit_subreddit() {
        let post = submission("t", 100, recent_unix(), false, false);
        let a = normalize(&post);
        assert_eq!(a.source, "Reddit r/stocks");
    }

    #[test]
    fn normalize_builds_full_reddit_url_from_permalink() {
        let post = submission("t", 100, recent_unix(), false, false);
        let a = normalize(&post);
        assert_eq!(
            a.url.as_deref(),
            Some("https://www.reddit.com/r/stocks/comments/abc/xyz/")
        );
    }

    #[test]
    fn normalize_published_at_is_rfc3339() {
        let post = submission("t", 100, 1_713_200_000.0, false, false);
        let a = normalize(&post);
        chrono::DateTime::parse_from_rfc3339(&a.published_at)
            .expect("published_at must be RFC3339");
        assert!(a.published_at.contains('T'));
    }

    #[test]
    fn normalize_link_post_empty_selftext_produces_empty_snippet() {
        let mut post = submission("t", 100, recent_unix(), false, false);
        post.selftext = String::new();
        let a = normalize(&post);
        assert_eq!(a.snippet, "");
    }

    // ── provider behavior with stubbed client ────────────────────────────

    #[tokio::test]
    async fn fetch_short_circuits_on_denylist() {
        let provider = RedditNewsProvider::new(RedditClient::for_test(), vec!["stocks".to_owned()]);
        let sym = Symbol::Equity(Ticker::parse("ALL").unwrap());
        let news = provider.fetch(&sym).await.expect("ok");
        assert!(news.articles.is_empty());
        assert!(news.summary.contains("ambiguous"));
    }

    #[tokio::test]
    async fn fetch_short_circuits_with_empty_subreddits() {
        let provider = RedditNewsProvider::new(RedditClient::for_test(), vec![]);
        let sym = Symbol::Equity(Ticker::parse("AAPL").unwrap());
        let news = provider.fetch(&sym).await.expect("ok");
        assert!(news.articles.is_empty());
        assert!(news.summary.contains("rollout gated"));
    }

    // ── full HTTP path with wiremock ─────────────────────────────────────

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn submission_json(title: &str, score: i64, permalink: &str, created_utc: f64) -> String {
        format!(
            r#"{{
                "kind": "t3",
                "data": {{
                    "title": "{title}",
                    "selftext": "body",
                    "permalink": "{permalink}",
                    "subreddit": "stocks",
                    "created_utc": {created_utc},
                    "score": {score},
                    "over_18": false,
                    "stickied": false
                }}
            }}"#
        )
    }

    fn listing_json(children: &[String]) -> String {
        format!(
            r#"{{ "kind": "Listing", "data": {{ "children": [{}] }} }}"#,
            children.join(",")
        )
    }

    #[tokio::test]
    async fn fetch_sorts_by_score_desc_and_caps_at_max_articles() {
        let now_unix = Utc::now().timestamp() as f64;
        let mut children = Vec::new();
        let extra = 5usize;
        let n = REDDIT_SENTIMENT_MAX_ARTICLES + extra;
        for i in 0..n {
            children.push(submission_json(
                &format!("post-{i}"),
                (REDDIT_MIN_SCORE as i64) + (i as i64) * 10,
                &format!("/r/stocks/comments/{i}/post/"),
                now_unix - (i as f64) * 60.0,
            ));
        }
        let body = listing_json(&children);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let client = RedditClient::new(
            http,
            crate::rate_limit::SharedRateLimiter::disabled("test-reddit"),
            "scorpio-analyst-test/0.0.0".to_owned(),
        )
        .with_base_url(server.uri());
        let provider = RedditNewsProvider::new(client, vec!["stocks".to_owned()]);
        let sym = Symbol::Equity(Ticker::parse("AAPL").unwrap());

        let news = provider.fetch(&sym).await.expect("ok");
        assert_eq!(news.articles.len(), REDDIT_SENTIMENT_MAX_ARTICLES);

        let first = news.articles.first().unwrap();
        assert_eq!(first.title, format!("post-{}", n - 1));

        assert!(
            news.articles
                .iter()
                .all(|a| a.source.starts_with("Reddit r/"))
        );
    }
}
