//! Equity-pack analyst implementations.
//!
//! These are the four analysts that ship today and that the baseline pack
//! fans out across: fundamentals (Finnhub), sentiment (Finnhub news),
//! news (Finnhub + FRED), technical (Yahoo Finance OHLCV + kand). All four
//! share the retry / prompt / tool-use plumbing in [`common`].
pub(crate) mod common;
mod fundamental;
mod news;
mod sentiment;
mod technical;

pub use fundamental::FundamentalAnalyst;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use fundamental::build_fundamental_system_prompt;
pub use news::NewsAnalyst;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use news::build_news_system_prompt;
pub use sentiment::SentimentAnalyst;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use sentiment::build_sentiment_system_prompt;
pub use technical::TechnicalAnalyst;
#[cfg(any(test, feature = "test-helpers"))]
pub(crate) use technical::build_technical_system_prompt;
