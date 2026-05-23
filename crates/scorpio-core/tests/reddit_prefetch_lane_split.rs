//! Integration tests for the Reddit sentiment-sidecar lane-split contract.

use std::time::Duration;

use async_trait::async_trait;
use scorpio_core::{
    agents::analyst::prefetch_analyst_news,
    analysis_packs::{RuntimePolicy, resolve_runtime_policy},
    data::traits::NewsProvider,
    domain::Symbol,
    error::TradingError,
    state::{NewsArticle, NewsData},
};

/// Tiny stub provider mirroring the in-crate test helper.
struct StubNewsProvider {
    result: Result<NewsData, TradingError>,
    name: &'static str,
}

impl StubNewsProvider {
    fn ok(name: &'static str, data: NewsData) -> Self {
        Self {
            result: Ok(data),
            name,
        }
    }
    fn err(name: &'static str) -> Self {
        Self {
            result: Err(TradingError::NetworkTimeout {
                elapsed: Duration::ZERO,
                message: format!("{name} simulated failure"),
            }),
            name,
        }
    }
}

#[async_trait]
impl NewsProvider for StubNewsProvider {
    fn provider_name(&self) -> &'static str {
        self.name
    }
    async fn fetch(&self, _symbol: &Symbol) -> Result<NewsData, TradingError> {
        match &self.result {
            Ok(d) => Ok(d.clone()),
            Err(e) => Err(match e {
                TradingError::NetworkTimeout { elapsed, message } => TradingError::NetworkTimeout {
                    elapsed: *elapsed,
                    message: message.clone(),
                },
                other => panic!("unexpected stub error: {other:?}"),
            }),
        }
    }
}

fn article(title: &str, source: &str, published_at: &str) -> NewsArticle {
    NewsArticle {
        title: title.to_owned(),
        source: source.to_owned(),
        published_at: published_at.to_owned(),
        relevance_score: None,
        snippet: String::new(),
        url: Some(format!("https://example.com/{}", title.replace(' ', "-"))),
    }
}

fn news(articles: Vec<NewsArticle>) -> NewsData {
    NewsData {
        articles,
        macro_events: vec![],
        summary: String::new(),
    }
}

#[tokio::test]
async fn vetted_lane_never_contains_reddit_sources() {
    let fh = news(vec![article(
        "Reuters Article",
        "Reuters",
        "2026-03-14T12:00:00Z",
    )]);
    let yf = news(vec![article(
        "Yahoo Article",
        "Yahoo",
        "2026-03-14T11:00:00Z",
    )]);
    let rd = news(vec![article(
        "Reddit Article",
        "Reddit r/stocks",
        "2026-03-14T10:00:00Z",
    )]);

    let bundle = prefetch_analyst_news(
        &StubNewsProvider::ok("finnhub", fh),
        &StubNewsProvider::ok("yfinance", yf),
        &StubNewsProvider::ok("reddit", rd),
        "AAPL",
    )
    .await;

    let vetted = bundle.vetted.expect("vetted lane should exist");
    assert!(
        vetted
            .articles
            .iter()
            .all(|a| !a.source.starts_with("Reddit r/")),
        "vetted lane must never carry Reddit sources"
    );

    let sentiment = bundle.sentiment.expect("sentiment lane should exist");
    let reddit_in_sentiment = sentiment
        .articles
        .iter()
        .filter(|a| a.source.starts_with("Reddit r/"))
        .count();
    assert!(
        reddit_in_sentiment >= 1,
        "sentiment lane should carry Reddit rows"
    );
}

#[tokio::test]
async fn sentiment_only_when_reddit_alone_succeeds() {
    let rd = news(vec![article(
        "Reddit Article",
        "Reddit r/stocks",
        "2026-03-14T10:00:00Z",
    )]);

    let bundle = prefetch_analyst_news(
        &StubNewsProvider::err("finnhub"),
        &StubNewsProvider::err("yfinance"),
        &StubNewsProvider::ok("reddit", rd),
        "AAPL",
    )
    .await;

    assert!(bundle.vetted.is_none());
    assert!(bundle.sentiment.is_some());
}

#[tokio::test]
async fn sentiment_lane_preserves_reddit_row_when_url_matches_vetted_story() {
    let shared_url = "https://www.reddit.com/r/stocks/comments/abc/x/";
    let mut rd_art = article("X", "Reddit r/stocks", "2026-03-14T10:00:00Z");
    rd_art.url = Some(shared_url.to_owned());

    let mut yf_art = article("X", "Yahoo Finance", "2026-03-14T10:00:00Z");
    yf_art.url = Some(shared_url.to_owned());

    let bundle = prefetch_analyst_news(
        &StubNewsProvider::ok(
            "finnhub",
            NewsData {
                articles: vec![],
                macro_events: vec![],
                summary: String::new(),
            },
        ),
        &StubNewsProvider::ok("yfinance", news(vec![yf_art])),
        &StubNewsProvider::ok("reddit", news(vec![rd_art])),
        "AAPL",
    )
    .await;

    let sentiment = bundle.sentiment.expect("sentiment lane should exist");
    let count = sentiment
        .articles
        .iter()
        .filter(|a| a.url.as_deref() == Some(shared_url))
        .count();
    assert_eq!(
        count, 2,
        "sentiment lane must preserve the Reddit row even when URL/title overlap a vetted story"
    );
}

#[test]
fn baseline_equity_pack_carries_reddit_subreddits() {
    let p = resolve_runtime_policy("baseline").expect("baseline");
    assert!(p.reddit_subreddits.iter().any(|s| s == "stocks"));
    assert!(p.reddit_subreddits.iter().any(|s| s == "investing"));
}

#[test]
fn runtime_policy_serde_deserializes_without_reddit_subreddits() {
    let policy = resolve_runtime_policy("baseline").expect("baseline");
    let mut json: serde_json::Value = serde_json::to_value(&policy).expect("serialize");
    json.as_object_mut().unwrap().remove("reddit_subreddits");

    let back: RuntimePolicy =
        serde_json::from_value(json).expect("older snapshot must deserialize");
    assert!(back.reddit_subreddits.is_empty());
}
