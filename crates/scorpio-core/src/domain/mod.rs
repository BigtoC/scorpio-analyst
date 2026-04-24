//! Domain types shared by every asset-class pack.
//!
//! This module is the trunk on which the asset-class generalization rests.
//! It owns the typed identity layer ([`Symbol`], [`AssetClass`]) that analyst,
//! provider, and valuator phases will dispatch on in later phases of the
//! refactor. Keeping these types close to the crate root lets the rest of the
//! pipeline route without pulling in pack-specific modules.
pub mod class;
pub mod symbol;

#[cfg(test)]
mod tests;

pub use class::AssetClass;
pub use symbol::{CaipAssetId, Symbol, Ticker};
