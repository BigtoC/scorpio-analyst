//! ETF valuators.
//!
//! Phase 1: [`EtfPremiumDiscountValuator`] composes premium/discount band,
//! composition, and tracking error.

pub mod category_norms;
pub mod premium_discount;
// Tracking-error computation is not wired into the runtime in the current scope
// (benchmark daily OHLCV is intentionally unresolved). The pure function is kept
// and exercised by its own tests only, ready for a follow-on plan to re-enable.
#[cfg(test)]
mod tracking_error;

pub use premium_discount::EtfPremiumDiscountValuator;
