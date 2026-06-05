//! Google News RSS ingest — [`GnewsNewsProvider`].
//!
//! Wired into [`crate::agents::analyst::prefetch_analyst_news`] as a third
//! vetted news feed alongside Finnhub and Yahoo Finance. Queries Google News
//! RSS via [`gnews_rs::NewsClient::get_search`]; no API key is required.

pub mod news_provider;

pub use news_provider::GnewsNewsProvider;
