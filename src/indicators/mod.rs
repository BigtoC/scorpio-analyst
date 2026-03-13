//! Technical indicator calculation module.
//!
//! Exposes the `kand`-backed calculator and `rig` tool wrappers so the
//! downstream Technical Analyst agent can import everything through a single
//! module path:
//!
//! ```rust,ignore
//! use scorpio_analyst::indicators::{
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

pub mod refactored_calc;

pub use refactored_calc::{
    // Intermediate result types
    BollingerResult,
    // Tool structs
    CalculateAllIndicators,
    // Tool arg types
    CalculateAllIndicatorsArgs,
    CalculateAtr,
    CalculateAtrArgs,
    CalculateBollingerArgs,
    CalculateBollingerBands,
    CalculateIndicatorByName,
    CalculateIndicatorByNameArgs,
    CalculateMacd,
    CalculateMacdArgs,
    CalculateRsi,
    CalculateRsiArgs,
    MacdResult,
    NamedIndicatorOutput,
    // Calculation functions
    calculate_all_indicators,
    calculate_atr,
    calculate_bollinger_bands,
    calculate_ema,
    calculate_indicator_by_name,
    calculate_macd,
    calculate_rsi,
    calculate_sma,
    calculate_vwma,
    derive_support_resistance,
};
