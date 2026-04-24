//! Equity-pack analyst implementations.
//!
//! These are the four analysts that ship today and that the baseline pack
//! fans out across: fundamentals (Finnhub), sentiment (Finnhub news),
//! news (Finnhub + FRED), technical (Yahoo Finance OHLCV + kand). All four
//! share the retry / prompt / tool-use plumbing in [`common`].
pub(crate) mod common;
mod fundamental;
mod news;
mod prompt;
mod sentiment;
mod technical;

pub use fundamental::FundamentalAnalyst;
pub use news::NewsAnalyst;
pub use sentiment::SentimentAnalyst;
pub use technical::TechnicalAnalyst;
