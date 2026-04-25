//! Active-pack completeness validation.
//!
//! Lives separately from [`AnalysisPackManifest::validate`] (shape-only,
//! always run) and [`resolve_runtime_policy_for_manifest`] (transport-only,
//! must keep working for inactive stub packs). Active completeness is the
//! per-run question "does this pack carry every prompt slot the configured
//! topology will ask for?" and is invoked by `PreflightTask` once per cycle.

use crate::analysis_packs::manifest::{AnalysisPackManifest, PackId};
use crate::prompts::is_effectively_empty;
use crate::workflow::topology::{PromptSlot, RunRoleTopology, required_prompt_slots};

/// Slots required by the configured topology that the active pack does not
/// supply (or supplies as effectively-empty content).
///
/// Multi-slot ordering matches the iteration order of
/// `required_prompt_slots(topology)`, which is the stable `BTreeSet` order of
/// the underlying `PromptSlot` discriminants — so callers can rely on the
/// listing being deterministic across runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletenessError {
    pub pack_id: PackId,
    pub missing_slots: Vec<PromptSlot>,
}

impl std::fmt::Display for CompletenessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.missing_slots.iter().map(|s| s.name()).collect();
        write!(
            f,
            "active pack {:?} is missing {} required prompt slot(s): {}",
            self.pack_id,
            names.len(),
            names.join(", ")
        )
    }
}

impl std::error::Error for CompletenessError {}

/// Verify that the active pack populates every prompt slot the configured
/// topology will exercise.
///
/// Returns `Ok(())` when every required slot has meaningful content (per
/// `is_effectively_empty`), or `Err(CompletenessError)` listing every
/// missing slot in stable order so the diagnostic is the same on every run.
///
/// **Not invoked here:** this function does not call `manifest.validate()`
/// (shape-only checks) or `resolve_runtime_policy_for_manifest()`
/// (transport). It is purely the active-completeness gate that
/// `PreflightTask` runs against the active pack before any analyst or
/// model task fires.
pub fn validate_active_pack_completeness(
    manifest: &AnalysisPackManifest,
    topology: &RunRoleTopology,
) -> Result<(), CompletenessError> {
    let required = required_prompt_slots(topology);
    let mut missing: Vec<PromptSlot> = Vec::new();
    for slot in required {
        let content = slot.read(&manifest.prompt_bundle);
        if is_effectively_empty(content) {
            missing.push(slot);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(CompletenessError {
            pack_id: manifest.id,
            missing_slots: missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::analysis_packs::resolve_pack;
    use crate::prompts::PromptBundle;
    use crate::workflow::topology::build_run_topology;

    fn fully_enabled_baseline_topology() -> RunRoleTopology {
        let manifest = resolve_pack(PackId::Baseline);
        build_run_topology(&manifest.required_inputs, 2, 2)
    }

    #[test]
    fn baseline_pack_is_complete_under_fully_enabled_topology() {
        let manifest = resolve_pack(PackId::Baseline);
        let topology = fully_enabled_baseline_topology();
        let result = validate_active_pack_completeness(&manifest, &topology);
        assert!(
            result.is_ok(),
            "baseline pack should be complete: {result:?}"
        );
    }

    #[test]
    fn fully_empty_bundle_reports_thirteen_missing_slots() {
        let mut manifest = resolve_pack(PackId::Baseline);
        manifest.prompt_bundle = PromptBundle::empty();
        let topology = fully_enabled_baseline_topology();
        let err = validate_active_pack_completeness(&manifest, &topology)
            .expect_err("empty bundle must fail completeness");
        assert_eq!(err.missing_slots.len(), 13);
        assert_eq!(err.pack_id, PackId::Baseline);
    }

    #[test]
    fn whitespace_only_slot_is_reported_as_missing() {
        let mut manifest = resolve_pack(PackId::Baseline);
        manifest.prompt_bundle.fundamental_analyst = Cow::Borrowed("   \t\n");
        let topology = fully_enabled_baseline_topology();
        let err = validate_active_pack_completeness(&manifest, &topology)
            .expect_err("whitespace-only slot must fail");
        assert_eq!(err.missing_slots, vec![PromptSlot::FundamentalAnalyst]);
    }

    #[test]
    fn placeholder_only_slot_is_reported_as_missing() {
        let mut manifest = resolve_pack(PackId::Baseline);
        // Replace the trader slot with placeholder-only content. After
        // substitution this would render to a degenerate prompt.
        manifest.prompt_bundle.trader = Cow::Borrowed("{ticker} {current_date}");
        let topology = fully_enabled_baseline_topology();
        let err = validate_active_pack_completeness(&manifest, &topology)
            .expect_err("placeholder-only slot must fail");
        assert_eq!(err.missing_slots, vec![PromptSlot::Trader]);
    }

    #[test]
    fn missing_slots_listed_in_stable_order() {
        // Two slots missing, picked from non-adjacent BTreeSet positions to
        // catch implementations that depend on insertion order.
        let mut manifest = resolve_pack(PackId::Baseline);
        manifest.prompt_bundle.fundamental_analyst = Cow::Borrowed("");
        manifest.prompt_bundle.fund_manager = Cow::Borrowed("");
        let topology = fully_enabled_baseline_topology();
        let err =
            validate_active_pack_completeness(&manifest, &topology).expect_err("two missing slots");
        // BTreeSet order matches PromptSlot variant declaration order:
        // FundamentalAnalyst comes before FundManager.
        assert_eq!(
            err.missing_slots,
            vec![PromptSlot::FundamentalAnalyst, PromptSlot::FundManager]
        );
    }

    #[test]
    fn zero_round_topology_only_validates_required_subset() {
        // Empty bundle with zero-rounds topology: only analyst + trader +
        // fund_manager slots are required, so the error lists 6 slots, not 13.
        let mut manifest = resolve_pack(PackId::Baseline);
        manifest.prompt_bundle = PromptBundle::empty();
        let topology = build_run_topology(&manifest.required_inputs, 0, 0);
        let err = validate_active_pack_completeness(&manifest, &topology)
            .expect_err("empty bundle, six required slots");
        assert_eq!(err.missing_slots.len(), 6);
        // None of the debate or risk slots should be in the error.
        assert!(!err.missing_slots.contains(&PromptSlot::BullishResearcher));
        assert!(!err.missing_slots.contains(&PromptSlot::AggressiveRisk));
    }

    #[test]
    fn display_formats_missing_slots_human_readable() {
        let err = CompletenessError {
            pack_id: PackId::Baseline,
            missing_slots: vec![PromptSlot::Trader, PromptSlot::FundManager],
        };
        let formatted = format!("{err}");
        assert!(formatted.contains("trader"));
        assert!(formatted.contains("fund_manager"));
        assert!(formatted.contains("2 required prompt slot"));
    }
}
