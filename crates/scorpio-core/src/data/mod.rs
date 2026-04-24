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
//! | [`fetch_vix_data`] | Fetch and compute VIX market volatility snapshot |
//! | [`get_latest_close`] | Fetch the most recent closing price for a symbol |
//! | [`ResolvedInstrument`] | Canonical instrument identity record |
//! | [`resolve_symbol`] | Validate and canonicalize a raw ticker string |
//! | [`adapters`] | Enrichment adapter contracts, [`ProviderCapabilities`], and concrete providers |

pub mod adapters;
pub mod entity;
pub mod finnhub;
pub mod fred;
mod provider_impls;
pub mod routing;
pub mod symbol;
pub mod traits;
pub mod yfinance;

pub use entity::{ResolvedInstrument, resolve_symbol};
pub use finnhub::{
    FinnhubClient, GetCachedNews, GetEarnings, GetFundamentals, GetInsiderTransactions,
    GetMarketNews, GetNews, SymbolArgs,
};
pub use fred::{FredClient, GetEconomicIndicators};
#[cfg(test)]
pub use yfinance::StubbedFinancialResponses;
pub use yfinance::{
    Candle, GetOhlcv, OhlcvArgs, OhlcvToolContext, YFinanceClient, fetch_vix_data, get_latest_close,
};
