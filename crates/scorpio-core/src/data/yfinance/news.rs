//! Yahoo Finance company news provider.
//!
//! Fetches up-to-date company news articles for a symbol via the `yfinance-rs`
//! `NewsBuilder` and normalizes them into the shared [`NewsData`] shape.
//!
//! ## Normalization contract
//!
//! | Upstream field                         | Domain field          | Notes                                |
//! |----------------------------------------|-----------------------|--------------------------------------|
//! | `yfinance_rs::news::NewsArticle.title` | `NewsArticle.title`   | passed through as-is                 |
//! | `yfinance_rs::news::NewsArticle.publisher` | `NewsArticle.source` | `None` becomes `"Unknown"`          |
//! | `yfinance_rs::news::NewsArticle.published_at` | `NewsArticle.published_at` | `DateTime<Utc>` → RFC3339 string |
//! | `yfinance_rs::news::NewsArticle.link`  | `NewsArticle.url`     | `None` is preserved as `None`        |
//! | (n/a)                                  | `NewsArticle.snippet` | always `""` — Yahoo does not supply  |
//! | (n/a)                                  | `macro_events`        | always empty — Yahoo has no macro events |

use chrono::Utc;
use yfinance_rs::NewsBuilder;

use super::ohlcv::YFinanceClient;
use crate::constants::NEWS_ANALYSIS_DAYS;
use crate::error::TradingError;
use crate::state::{NewsArticle, NewsData};

// ─── YFinanceNewsProvider ────────────────────────────────────────────────────

const NEWS_YAHOO_FETCH_LIMIT: u32 = 50;

/// Fetches and normalizes company news from Yahoo Finance.
///
/// Articles outside the [`NEWS_ANALYSIS_DAYS`] window are filtered out so
/// that the resulting [`NewsData`] covers the same time horizon as the
/// Finnhub news provider.
pub struct YFinanceNewsProvider {
    client: YFinanceClient,
}

impl YFinanceNewsProvider {
    /// Create a new provider from an existing [`YFinanceClient`].
    ///
    /// Clones the client so the provider shares the same HTTP connection
    /// pool and rate limiter.
    #[must_use]
    pub fn new(client: &YFinanceClient) -> Self {
        Self {
            client: client.clone(),
        }
    }

    /// Fetch the most recent company news for `symbol` and return a
    /// normalized [`NewsData`] covering the last [`NEWS_ANALYSIS_DAYS`] days.
    ///
    /// # Errors
    ///
    /// Returns `TradingError::NetworkTimeout` on HTTP failures or
    /// `TradingError::SchemaViolation` on parse failures — matching the
    /// error taxonomy used by [`super::ohlcv::map_yf_err`].
    pub async fn get_company_news(&self, symbol: &str) -> Result<NewsData, TradingError> {
        #[cfg(test)]
        if let Some(stubbed) = &self.client.stubbed_financials {
            if let Some(ref err_msg) = stubbed.news_error {
                return Err(TradingError::NetworkTimeout {
                    elapsed: std::time::Duration::ZERO,
                    message: err_msg.clone(),
                });
            }
            let articles = stubbed.news.clone().unwrap_or_default();
            return Ok(build_yahoo_news_data(symbol, articles));
        }

        self.client.session.limiter().acquire().await;

        let raw_articles = NewsBuilder::new(self.client.session.client(), symbol)
            .count(NEWS_YAHOO_FETCH_LIMIT)
            .fetch()
            .await
            .map_err(super::ohlcv::map_yf_err)?;

        Ok(build_yahoo_news_data(symbol, raw_articles))
    }
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn build_yahoo_news_data(
    symbol: &str,
    raw_articles: Vec<yfinance_rs::news::NewsArticle>,
) -> NewsData {
    let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;

    let articles: Vec<NewsArticle> = raw_articles
        .into_iter()
        .filter(|a| a.published_at >= cutoff)
        .map(normalize_yahoo_article)
        .collect();

    let article_count = articles.len();

    NewsData {
        articles,
        macro_events: vec![],
        summary: format!("Yahoo Finance: {article_count} articles for {symbol}"),
    }
}

fn normalize_yahoo_article(a: yfinance_rs::news::NewsArticle) -> NewsArticle {
    NewsArticle {
        title: a.title,
        source: a.publisher.unwrap_or_else(|| "Unknown".to_owned()),
        published_at: a.published_at.to_rfc3339(),
        relevance_score: None,
        snippet: String::new(),
        url: a.link,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::data::yfinance::ohlcv::{StubbedFinancialResponses, YFinanceClient};

    fn make_article(
        uuid: &str,
        title: &str,
        publisher: Option<&str>,
        link: Option<&str>,
        published_at: DateTime<Utc>,
    ) -> yfinance_rs::news::NewsArticle {
        yfinance_rs::news::NewsArticle {
            uuid: uuid.to_owned(),
            title: title.to_owned(),
            publisher: publisher.map(str::to_owned),
            link: link.map(str::to_owned),
            published_at,
        }
    }

    fn provider_with_stub(stub: StubbedFinancialResponses) -> YFinanceNewsProvider {
        let client = YFinanceClient::with_stubbed_financials(stub);
        YFinanceNewsProvider::new(&client)
    }

    #[tokio::test]
    async fn fetches_and_normalizes_articles() {
        let now = Utc::now();
        let articles = vec![make_article(
            "uuid-1",
            "AAPL Surges on Strong Earnings",
            Some("Reuters"),
            Some("https://example.com/aapl-news"),
            now,
        )];

        let provider = provider_with_stub(StubbedFinancialResponses {
            news: Some(articles),
            ..StubbedFinancialResponses::default()
        });

        let result = provider.get_company_news("AAPL").await.unwrap();

        assert_eq!(result.articles.len(), 1, "should return 1 article");

        let article = &result.articles[0];
        chrono::DateTime::parse_from_rfc3339(&article.published_at)
            .expect("published_at must be RFC3339");
        assert!(
            article.published_at.contains('T'),
            "RFC3339 must have 'T' separator"
        );
        assert_eq!(
            article.url,
            Some("https://example.com/aapl-news".to_owned()),
            "url must be populated from upstream link field"
        );
        assert_eq!(
            article.snippet, "",
            "snippet must be empty for Yahoo articles"
        );
        assert!(
            result.macro_events.is_empty(),
            "macro_events must be empty for Yahoo news provider"
        );
        assert_eq!(article.source, "Reuters");
    }

    #[tokio::test]
    async fn empty_feed_returns_empty_news_data() {
        let provider = provider_with_stub(StubbedFinancialResponses {
            news: Some(vec![]),
            ..StubbedFinancialResponses::default()
        });

        let result = provider.get_company_news("AAPL").await.unwrap();

        assert!(result.articles.is_empty(), "articles must be empty");
        assert!(result.macro_events.is_empty(), "macro_events must be empty");
        assert!(
            result.summary.contains("0 articles"),
            "summary should mention 0 articles; got: {}",
            result.summary
        );
    }

    #[tokio::test]
    async fn articles_outside_analysis_window_are_filtered_out() {
        let old_date = Utc::now() - chrono::Duration::days(60);
        let recent_date = Utc::now() - chrono::Duration::days(5);

        let provider = provider_with_stub(StubbedFinancialResponses {
            news: Some(vec![
                make_article("old", "Old News", None, None, old_date),
                make_article("recent", "Recent News", None, None, recent_date),
            ]),
            ..StubbedFinancialResponses::default()
        });

        let result = provider.get_company_news("AAPL").await.unwrap();

        assert_eq!(
            result.articles.len(),
            1,
            "only 1 article should survive the window filter"
        );
        assert_eq!(result.articles[0].title, "Recent News");
    }

    #[test]
    fn normalize_yahoo_article_maps_link_to_url() {
        let now = Utc::now();
        let raw = make_article(
            "u1",
            "Title",
            Some("Bloomberg"),
            Some("https://bloomberg.com/1"),
            now,
        );
        let normalized = normalize_yahoo_article(raw);
        assert_eq!(
            normalized.url,
            Some("https://bloomberg.com/1".to_owned()),
            "link must map to url"
        );
    }

    #[test]
    fn normalize_yahoo_article_none_publisher_becomes_unknown() {
        let now = Utc::now();
        let raw = make_article("u2", "Title", None, None, now);
        let normalized = normalize_yahoo_article(raw);
        assert_eq!(normalized.source, "Unknown");
    }
}
