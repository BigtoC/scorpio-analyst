//! Equity-pack valuators.
//!
//! Today there's one strategy — [`EquityDefaultValuator`] — which delegates
//! to [`crate::state::derive_valuation`]. Splitting out per-metric
//! valuators (DCF, multiples) is a follow-up; the shim keeps Phase 5
//! behaviour byte-identical.
mod default;

pub use default::EquityDefaultValuator;
