//! Yahoo Finance data submodule.
//!
//! All Yahoo Finance related fetching lives in [`ohlcv`]:
//!
//! | Export | Description |
//! |--------|-------------|
//! | [`YFinanceClient`] | Thin async wrapper around `yfinance-rs` with in-memory caching |
//! | [`Candle`] | Plain-`f64` daily OHLCV bar |
//! | [`GetOhlcv`] / [`OhlcvArgs`] / [`OhlcvToolContext`] | `rig` tool plumbing |
//! | [`fetch_vix_data`] | CBOE VIX market volatility snapshot |

pub mod ohlcv;

pub use ohlcv::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient, fetch_vix_data};
