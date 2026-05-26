//! Reddit news ingest — anonymous JSON HTTP client + sentiment-sidecar
//! [`NewsProvider`].
//!
//! Reddit is wired into [`crate::agents::analyst::prefetch_analyst_news`] as
//! a third provider, but in v1 its output only feeds the sentiment lane
//! ([`crate::agents::analyst::equity::SentimentAnalyst`]); the vetted lane
//! ([`crate::agents::analyst::equity::NewsAnalyst`]) stays on Finnhub + Yahoo.

pub mod client;
pub mod news_provider;
pub mod types;

pub use client::RedditClient;
pub use news_provider::RedditNewsProvider;
