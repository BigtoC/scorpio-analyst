//! ETF baseline pack — premium/discount + composition + tracking.
//!
//! Phase 1: yfinance quote + fund info + SEC EDGAR N-PORT-P + source-provided
//! benchmark OHLCV. Phase 2 (dealer GEX) is deferred.

mod baseline;

pub(crate) use baseline::append_leverage_warning_if_needed;
pub use baseline::etf_baseline_pack;
