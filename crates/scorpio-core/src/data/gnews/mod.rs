//! Google News RSS ingest — [`GnewsNewsProvider`].
//!
//! Wired into [`crate::agents::analyst::prefetch_analyst_news`] as a third
//! vetted news feed alongside Finnhub and Yahoo Finance. Issues a single
//! request to `news.google.com/rss/search?q=<ticker>` and parses the feed with
//! the `rss` crate; no API key is required.

pub mod news_provider;

pub use news_provider::GnewsNewsProvider;
