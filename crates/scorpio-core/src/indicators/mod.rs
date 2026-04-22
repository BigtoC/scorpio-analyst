//! Technical indicator calculation module.
//!
//! Exposes the `kand`-backed calculator and `rig` tool wrappers so the
//! downstream Technical Analyst agent can import everything through a single
//! module path:
//!
//! ```rust,ignore
//! use scorpio_core::indicators::{
//!     calculate_all_indicators, calculate_rsi, calculate_macd,
//!     calculate_atr, calculate_bollinger_bands, calculate_sma,
//!     calculate_ema, calculate_vwma, calculate_indicator_by_name,
//!     derive_support_resistance,
//!     MacdResult, BollingerResult, NamedIndicatorOutput,
//!     CalculateAllIndicators, CalculateRsi, CalculateMacd,
//!     CalculateAtr, CalculateBollingerBands, CalculateIndicatorByName,
//!     CalculateAllIndicatorsArgs, CalculateRsiArgs, CalculateMacdArgs,
//!     CalculateAtrArgs, CalculateBollingerArgs, CalculateIndicatorByNameArgs,
//! };
//! ```

mod batch;
mod core_math;
mod support_resistance;
mod tools;
mod types;
mod utils;

#[cfg(test)]
pub mod test_utils;

pub use batch::{calculate_all_indicators, calculate_indicator_by_name};
pub use core_math::{
    calculate_atr, calculate_bollinger_bands, calculate_ema, calculate_macd, calculate_rsi,
    calculate_sma, calculate_vwma,
};
pub use support_resistance::derive_support_resistance;
pub use tools::{
    CalculateAllIndicators, CalculateAllIndicatorsArgs, CalculateAtr, CalculateAtrArgs,
    CalculateBollingerArgs, CalculateBollingerBands, CalculateIndicatorByName,
    CalculateIndicatorByNameArgs, CalculateMacd, CalculateMacdArgs, CalculateRsi, CalculateRsiArgs,
};
pub use types::{BollingerResult, MacdResult, NamedIndicatorOutput};
