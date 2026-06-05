//! [`GnewsNewsProvider`] — Google News RSS vetted news feed.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gnews_rs::NewsClient;

use crate::{
    constants::{NEWS_ANALYSIS_DAYS, NEWS_SNIPPET_MAX_CHARS, NEWS_TITLE_MAX_CHARS},
    data::traits::NewsProvider,
    domain::Symbol,
    error::TradingError,
    state::{NewsArticle, NewsData},
};

/// Google News RSS news provider.
///
/// Queries `news.google.com/rss/search?q=<ticker>` via `gnews_rs`. Requires no
/// API key. Only equity symbols produce a meaningful search; non-equity symbols
/// return empty `NewsData` immediately.
#[derive(Clone, Debug, Default)]
pub struct GnewsNewsProvider;

impl GnewsNewsProvider {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn ticker_for_search(symbol: &Symbol) -> Option<String> {
        symbol.as_equity().map(|t| t.as_str().to_owned())
    }
}

/// Extract the host component from a URL string for display as the article source.
fn extract_host(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .to_owned()
}

/// Strip HTML tags from a string.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// Parse an RFC 2822 date string to RFC 3339 UTC.
fn parse_pub_date(pub_date: &str) -> Option<String> {
    DateTime::parse_from_rfc2822(pub_date)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[async_trait]
impl NewsProvider for GnewsNewsProvider {
    fn provider_name(&self) -> &'static str {
        "gnews"
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<NewsData, TradingError> {
        let Some(ticker) = Self::ticker_for_search(symbol) else {
            return Ok(NewsData {
                articles: vec![],
                macro_events: vec![],
                summary: "gnews: 0 articles (unsupported symbol shape)".to_owned(),
            });
        };

        // gnews_rs futures are not Send (rss::Channel::read_from returns
        // Box<dyn Error>, which is !Send). Run in a blocking thread with its
        // own runtime so the outer async context stays Send-safe.
        let raw = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new()
                .map(|rt| rt.block_on(NewsClient::get_search(&ticker)))
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

        let cutoff = Utc::now() - NEWS_ANALYSIS_DAYS;
        let articles: Vec<NewsArticle> = raw
            .iter()
            .filter_map(|item| {
                let published_at = parse_pub_date(&item.pub_date)?;
                let dt = DateTime::parse_from_rfc3339(&published_at)
                    .ok()?
                    .with_timezone(&Utc);
                if dt < cutoff {
                    return None;
                }
                let url = if item.origin_link.is_empty() {
                    None
                } else {
                    Some(item.origin_link.clone())
                };
                let source = if item.source.is_empty() {
                    url.as_deref()
                        .map(extract_host)
                        .unwrap_or_else(|| "Google News".to_owned())
                } else {
                    extract_host(&item.source)
                };
                Some(NewsArticle {
                    title: truncate_chars(&item.title, NEWS_TITLE_MAX_CHARS),
                    source,
                    published_at,
                    relevance_score: None,
                    snippet: truncate_chars(&strip_html(&item.description), NEWS_SNIPPET_MAX_CHARS),
                    url,
                })
            })
            .collect();

        let count = articles.len();
        Ok(NewsData {
            articles,
            macro_events: vec![],
            summary: format!("gnews: {count} articles"),
        })
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(
            extract_host("https://www.bloomberg.com/news/article"),
            "www.bloomberg.com"
        );
        assert_eq!(extract_host("http://reuters.com/"), "reuters.com");
        assert_eq!(extract_host("bloomberg.com"), "bloomberg.com");
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<p>Hello world</p>"), "Hello world");
        assert_eq!(strip_html("<a href=\"x\">click</a>"), "click");
        assert_eq!(strip_html("plain text"), "plain text");
    }

    #[test]
    fn parse_pub_date_rfc2822_to_rfc3339() {
        let input = "Thu, 05 Jun 2025 10:00:00 GMT";
        let result = parse_pub_date(input).expect("valid RFC 2822");
        assert!(result.contains("2025-06-05"), "expected date in {result}");
        DateTime::parse_from_rfc3339(&result).expect("output must be valid RFC 3339");
    }

    #[test]
    fn parse_pub_date_returns_none_for_invalid_input() {
        assert!(parse_pub_date("not a date").is_none());
        assert!(parse_pub_date("").is_none());
    }

    #[tokio::test]
    async fn fetch_returns_empty_for_non_equity_symbol() {
        use crate::domain::CaipAssetId;
        let json = r#"{"crypto":"eip155:1/slip44:60"}"#;
        let sym: Symbol = serde_json::from_str(json).expect("valid symbol json");
        let provider = GnewsNewsProvider::new();
        let news = provider.fetch(&sym).await.expect("ok");
        assert!(news.articles.is_empty());
        assert!(news.summary.contains("unsupported symbol shape"));
        // Type-guard so unused imports survive.
        let _unused: Option<CaipAssetId> = None;
    }
}
