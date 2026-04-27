//! Yahoo Finance data sub-module.
//!
//! Organised into focused layers:
//!
//! | Sub-module    | Description |
//! |---------------|-------------|
//! | [`client`]    | [`YfSession`] — shared `YfClient` + rate limiter used by all sibling modules |
//! | [`ohlcv`]     | [`YFinanceClient`], [`Candle`], [`GetOhlcv`], [`OhlcvToolContext`] — raw OHLCV fetcher and `rig` tool plumbing |
//! | [`price`]     | [`get_latest_close`], [`fetch_vix_data`] — derived price queries over `YFinanceClient` |
//! | [`financials`]| Quarterly financial statement, earnings trend, and profile fetchers |

mod client;
pub mod financials;
pub mod news;
pub mod ohlcv;
pub mod options;
pub mod price;

pub use news::YFinanceNewsProvider;
#[cfg(test)]
pub use ohlcv::StubbedFinancialResponses;
pub use ohlcv::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient};
pub use options::{GetOptionsSnapshot, YFinanceOptionsProvider};
pub use price::{fetch_vix_data, get_latest_close};
