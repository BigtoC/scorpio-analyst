//! Yahoo Finance data sub-module.
//!
//! Organised into two focused layers:
//!
//! | Sub-module | Description |
//! |------------|-------------|
//! | [`ohlcv`]  | [`YFinanceClient`], [`Candle`], [`GetOhlcv`], [`OhlcvToolContext`] — raw OHLCV fetcher and `rig` tool plumbing |
//! | [`price`]  | [`get_latest_close`], [`fetch_vix_data`] — derived price queries over `YFinanceClient` |

pub mod ohlcv;
pub mod price;

pub use ohlcv::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient};
pub use price::{fetch_vix_data, get_latest_close};
