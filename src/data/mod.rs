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
//! | [`GetMarketNews`] | `rig` tool: fetch general market news |
//! | [`GetEconomicIndicators`] | `rig` tool: fetch macro-economic indicator snapshot |
//! | [`YFinanceClient`] | Async wrapper for Yahoo Finance OHLCV data |
//! | [`Candle`] | Plain-`f64` OHLCV bar |
//! | [`GetOhlcv`] | `rig` tool: fetch historical OHLCV bars |
//! | [`OhlcvToolContext`] | Shared analysis-scoped OHLCV cache for technical tools |

pub mod finnhub;
mod symbol;
pub mod yfinance;

pub use finnhub::{
    FinnhubClient, GetEarnings, GetEconomicIndicators, GetFundamentals, GetInsiderTransactions,
    GetMarketNews, GetNews, SymbolArgs,
};
pub use yfinance::{Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient};
