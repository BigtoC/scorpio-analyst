//! Asset class taxonomy for routing analyst / provider / valuator selection.
//!
//! Kept as a narrow enum rather than a string so dispatch sites stay
//! compile-time exhaustive. Marked `#[non_exhaustive]` so future asset-class
//! additions (commodities, FX) remain a non-breaking change for downstream
//! external packs.
use serde::{Deserialize, Serialize};

/// Top-level asset class a [`super::Symbol`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssetClass {
    /// Listed equity instruments — stocks, ETFs, indices.
    Equity,
    /// Crypto-native assets addressable by CAIP-10 identifiers.
    Crypto,
}

impl AssetClass {
    /// Resolve the asset class from a [`super::Symbol`] variant.
    #[must_use]
    pub fn of(symbol: &super::Symbol) -> Self {
        match symbol {
            super::Symbol::Equity(_) => Self::Equity,
            super::Symbol::Crypto(_) => Self::Crypto,
        }
    }
}
