//! Yahoo Finance data sub-module.
//!
//! Organises all Yahoo Finance related fetching into two focused files:
//!
//! | Sub-module | Description |
//! |------------|-------------|
//! | [`ohlcv`] | [`YFinanceClient`], [`Candle`], [`GetOhlcv`], [`OhlcvToolContext`] — historical price bars |
//! | [`vix`] | [`fetch_vix_data`] — CBOE VIX market volatility snapshot |

pub mod ohlcv;
pub mod vix;

pub use ohlcv::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient};
pub use vix::fetch_vix_data;
