//! Inner facade for the `refactored_calc` submodule directory.
//!
//! Wires together the five focused submodules and re-exports every public
//! symbol so the outer `indicators::mod` facade can forward them unchanged.

mod batch;
mod core_math;
mod support_resistance;
mod tools;
mod types;
mod utils;

// ── Re-export public types ────────────────────────────────────────────────────

pub use types::{BollingerResult, MacdResult, NamedIndicatorOutput};

// ── Re-export calculation functions ──────────────────────────────────────────

pub use batch::{calculate_all_indicators, calculate_indicator_by_name};
pub use core_math::{
    calculate_atr, calculate_bollinger_bands, calculate_ema, calculate_macd, calculate_rsi,
    calculate_sma, calculate_vwma,
};
pub use support_resistance::derive_support_resistance;

// ── Re-export rig tool structs and their args ─────────────────────────────────

pub use tools::{
    CalculateAllIndicators, CalculateAllIndicatorsArgs, CalculateAtr, CalculateAtrArgs,
    CalculateBollingerArgs, CalculateBollingerBands, CalculateIndicatorByName,
    CalculateIndicatorByNameArgs, CalculateMacd, CalculateMacdArgs, CalculateRsi, CalculateRsiArgs,
};
