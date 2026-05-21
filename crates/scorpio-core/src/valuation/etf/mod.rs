//! ETF valuators.
//!
//! Phase 1: [`EtfPremiumDiscountValuator`] composes premium/discount band,
//! composition, and tracking error.

pub mod category_norms;
pub mod premium_discount;
pub mod tracking_error;

pub use premium_discount::EtfPremiumDiscountValuator;
