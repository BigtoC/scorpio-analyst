//! Pack registry — maps a [`PackId`] to its concrete [`AnalysisPackManifest`].
//!
//! Phase 7 split the single `builtin.rs` into this registry plus the
//! per-asset-class manifest modules under `equity/` and `crypto/`. The
//! registry is the single entry point for runtime pack resolution;
//! downstream code (`selection::resolve_runtime_policy`,
//! `workflow::builder::TradingPipeline::from_pack`) reaches for this
//! function rather than the per-pack factories directly.
use super::{AnalysisPackManifest, PackId, crypto, equity};

/// Resolve a [`PackId`] into its full [`AnalysisPackManifest`].
///
/// Pure compile-time lookup — no I/O, no filesystem loading in this slice.
/// External / runtime-loaded packs are an explicit follow-up.
#[must_use]
pub fn resolve_pack(id: PackId) -> AnalysisPackManifest {
    match id {
        PackId::Baseline => equity::baseline_pack(),
        // Registered but not user-selectable — see `PackId::from_str`.
        PackId::CryptoDigitalAsset => crypto::digital_asset_pack(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_baseline_returns_manifest_with_matching_id() {
        let pack = resolve_pack(PackId::Baseline);
        assert_eq!(pack.id, PackId::Baseline);
    }

    #[test]
    fn resolve_crypto_digital_asset_returns_matching_id_even_though_unselectable() {
        // The stub pack is registered for validation but can't be chosen via
        // `PackId::from_str`. This test proves the registry entry exists so
        // crypto-side code can still call `resolve_pack(PackId::CryptoDigitalAsset)`
        // directly (e.g. from tests or a future feature flag).
        let pack = resolve_pack(PackId::CryptoDigitalAsset);
        assert_eq!(pack.id, PackId::CryptoDigitalAsset);
        assert!(pack.validate().is_ok());
    }

    #[test]
    fn from_str_does_not_expose_crypto_digital_asset() {
        let err = "crypto_digital_asset"
            .parse::<PackId>()
            .expect_err("crypto pack must stay unselectable via config");
        assert!(err.contains("unknown analysis pack"));
    }
}
