//! Pack registry — maps a [`PackId`] to its concrete [`AnalysisPackManifest`].
//!
//! Phase 7 split the single `builtin.rs` into this registry plus the
//! per-asset-class manifest modules under `equity/` and `crypto/`. The
//! registry is the single entry point for runtime pack resolution;
//! downstream code (`selection::resolve_runtime_policy`,
//! `workflow::builder::TradingPipeline::from_pack`) reaches for this
//! function rather than the per-pack factories directly.
use tracing::info;

use super::completeness::{CompletenessError, validate_active_pack_completeness};
use super::{AnalysisPackManifest, PackId, crypto, equity};
use crate::workflow::build_run_topology;

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

/// Every registered pack identifier, in declaration order.
///
/// Used by [`init_diagnostics`] to enumerate packs without depending on
/// `strum` or a derive. Adding a new `PackId` variant requires extending
/// this array — the registry's `resolve_pack` match would otherwise
/// silently treat the new variant as unhandled.
const REGISTERED_PACKS: &[PackId] = &[PackId::Baseline, PackId::CryptoDigitalAsset];

/// Compute completeness diagnostics for every registered active pack.
///
/// Active = "has a non-empty prompt bundle". Stub packs that ship
/// `PromptBundle::empty()` (the inactive crypto stub today) are skipped so
/// they do not pollute startup logs. For each remaining pack we build the
/// fully-enabled would-be topology from the pack's declared
/// `required_inputs` (debate and risk both on) and run
/// [`validate_active_pack_completeness`] against it; the resulting errors
/// are the diagnostics emitted by [`init_diagnostics`].
///
/// This is the pure variant — no logging side effects — so it can be
/// asserted against in tests.
#[must_use]
pub fn pack_diagnostics() -> Vec<CompletenessError> {
    let mut errors = Vec::new();
    for &pack_id in REGISTERED_PACKS {
        let manifest = resolve_pack(pack_id);
        if manifest.prompt_bundle.is_empty() {
            // Stub-skip rule: the empty-bundle sentinel powers the inactive
            // crypto stub. Active packs never use `PromptBundle::empty()`.
            continue;
        }
        // Fully-enabled would-be topology: debate and risk both on, so the
        // diagnostic answers "what would this pack need if every optional
        // stage were enabled?" That's the most useful baseline-suitable
        // signal at startup time when no per-run round-count config exists.
        let topology = build_run_topology(&manifest.required_inputs, 1, 1);
        if let Err(err) = validate_active_pack_completeness(&manifest, &topology) {
            errors.push(err);
        }
    }
    errors
}

/// Emit non-blocking `info!` lines for every active pack that is missing
/// required prompt slots under the fully-enabled would-be topology.
///
/// Invoked once from `AnalysisRuntime::new` so the diagnostic does not fire
/// on every `resolve_pack` call. Stub packs (those whose prompt bundle is
/// `PromptBundle::empty()`) are skipped.
pub fn init_diagnostics() {
    for err in pack_diagnostics() {
        let slot_names: Vec<&'static str> =
            err.missing_slots.iter().map(|slot| slot.name()).collect();
        info!(
            pack_id = %err.pack_id,
            missing_slot_count = err.missing_slots.len(),
            missing_slots = ?slot_names,
            "active pack is missing prompt slots under fully-enabled topology"
        );
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

    #[test]
    fn registered_packs_match_resolve_pack_arms() {
        // Sanity: every entry in REGISTERED_PACKS must be resolvable.
        // Adding a new PackId without extending REGISTERED_PACKS would not
        // fail this test directly, but it would silently cause init_diagnostics
        // to skip the new pack — surface that intent here for the maintainer.
        for &pack_id in REGISTERED_PACKS {
            let manifest = resolve_pack(pack_id);
            assert_eq!(manifest.id, pack_id);
        }
        // Spot-check that both currently-known variants are listed.
        assert!(REGISTERED_PACKS.contains(&PackId::Baseline));
        assert!(REGISTERED_PACKS.contains(&PackId::CryptoDigitalAsset));
    }

    #[test]
    fn pack_diagnostics_skips_stub_pack_with_empty_bundle() {
        // The inactive crypto stub uses PromptBundle::empty() as its sentinel.
        // pack_diagnostics must skip it — otherwise the baseline-suitable
        // would-be topology would emit info noise on every startup.
        let crypto_manifest = resolve_pack(PackId::CryptoDigitalAsset);
        assert!(
            crypto_manifest.prompt_bundle.is_empty(),
            "fixture invariant: crypto stub still uses PromptBundle::empty()"
        );
        let errors = pack_diagnostics();
        // No diagnostic for the crypto pack id should appear in the output.
        assert!(
            !errors
                .iter()
                .any(|e| e.pack_id == PackId::CryptoDigitalAsset),
            "crypto stub should be skipped, got: {errors:?}"
        );
    }

    #[test]
    fn pack_diagnostics_for_complete_baseline_returns_empty() {
        // Baseline is complete under the fully-enabled topology — so no
        // diagnostics should be emitted for it.
        let errors = pack_diagnostics();
        assert!(
            !errors.iter().any(|e| e.pack_id == PackId::Baseline),
            "baseline should not produce diagnostics, got: {errors:?}"
        );
    }

    #[test]
    fn pack_diagnostics_returns_empty_today() {
        // Composite assertion: today, with baseline complete and crypto
        // skipped as a stub, init_diagnostics should be silent.
        assert!(pack_diagnostics().is_empty());
    }
}
