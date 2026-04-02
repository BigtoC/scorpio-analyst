//! Financial market data ingestion layer.
//!
//! This module exposes the Finnhub and Yahoo Finance client wrappers used by
//! downstream analyst agents to populate [`crate::state`] data structures.
//!
//! ## Re-exported types
//!
//! | Path | Description |
//! |------|-------------|
//! | [`FinnhubClient`] | Async wrapper for Finnhub fundamentals, earnings, news, and insider transactions |
//! | [`GetFundamentals`] | `rig` tool: fetch corporate fundamentals |
//! | [`GetEarnings`] | `rig` tool: fetch quarterly earnings |
//! | [`GetInsiderTransactions`] | `rig` tool: fetch insider transactions |
//! | [`GetNews`] | `rig` tool: fetch company news |
//! | [`GetCachedNews`] | `rig` tool: serve pre-fetched news from cache (avoids duplicate Finnhub call) |
//! | [`GetMarketNews`] | `rig` tool: fetch general market news |
//! | [`FredClient`] | Async wrapper for FRED macro-economic data |
//! | [`GetEconomicIndicators`] | `rig` tool: fetch macro-economic indicator snapshot from FRED |
//! | [`YFinanceClient`] | Async wrapper for Yahoo Finance OHLCV data |
//! | [`Candle`] | Plain-`f64` OHLCV bar |
//! | [`GetOhlcv`] | `rig` tool: fetch historical OHLCV bars |
//! | [`OhlcvToolContext`] | Shared analysis-scoped OHLCV cache for technical tools |

pub mod finnhub;
pub mod fred;
mod symbol;
pub mod yfinance;

pub use finnhub::{
    FinnhubClient, GetCachedNews, GetEarnings, GetFundamentals, GetInsiderTransactions,
    GetMarketNews, GetNews, SymbolArgs,
};
pub use fred::{FredClient, GetEconomicIndicators};
pub use yfinance::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient};
