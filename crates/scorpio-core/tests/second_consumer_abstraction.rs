#![cfg(feature = "test-helpers")]

//! Unit 5: Second-Consumer API-Shape Contract Test (R8).
//!
//! Constructs a synthetic non-baseline `AnalysisPackManifest` in test code
//! and asserts that the topology abstraction's currently-shipped API surface
//! (`validate_active_pack_completeness`, `required_prompt_slots`,
//! `build_run_topology`) accepts a non-baseline manifest shape and derives
//! the correct subsets across topology variants.
//!
//! **Honest framing:** this test verifies the topology functions are total
//! over the `Role` enum and accept a non-baseline manifest shape; it does
//! *not* empirically validate that the abstraction is right for real-world
//! second packs. A synthetic manifest authored alongside the topology
//! mapping cannot surface gaps the author didn't already imagine — real
//! fitness validation is deferred until a real second pack lands.
//!
//! The maximal-children fan-out claim from the plan (Unit 5 Approach claim
//! 4) still depends on Unit 4b's `RoutingFlags`-gated per-child no-op
//! machinery. This file therefore limits itself to the pre-4b API-shape
//! claims that are truthful on the current branch.

use std::borrow::Cow;
use std::collections::HashMap;

use scorpio_core::analysis_packs::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
    validate_active_pack_completeness,
};
use scorpio_core::prompts::PromptBundle;
use scorpio_core::testing::runtime_policy_from_manifest;
use scorpio_core::workflow::{PromptSlot, Role, build_run_topology, required_prompt_slots};

/// Build a synthetic non-baseline manifest with a one-role analyst roster
/// (`news` only) and a `PromptBundle` populated *only* for the slots that
/// `required_prompt_slots` will ask for under a zero-debate, zero-risk
/// topology. This is the API-shape contract test fixture: it shares no
/// prose with the baseline pack and is constructed entirely in test code.
fn synthetic_one_role_manifest() -> AnalysisPackManifest {
    let bundle = PromptBundle {
        // Only `news_analyst`, `trader`, and `fund_manager` are required
        // under a one-role / zero-rounds topology. Other slots stay empty
        // so we can prove `required_prompt_slots` correctly omits them.
        fundamental_analyst: Cow::Borrowed(""),
        sentiment_analyst: Cow::Borrowed(""),
        news_analyst: Cow::Borrowed(
            "Synthetic test news analyst for {ticker} on {current_date}.\n\nReport schema: ...",
        ),
        technical_analyst: Cow::Borrowed(""),
        bullish_researcher: Cow::Borrowed(""),
        bearish_researcher: Cow::Borrowed(""),
        debate_moderator: Cow::Borrowed(""),
        trader: Cow::Borrowed(
            "Synthetic test trader for {ticker} on {current_date}. Decide and rationalise.",
        ),
        aggressive_risk: Cow::Borrowed(""),
        conservative_risk: Cow::Borrowed(""),
        neutral_risk: Cow::Borrowed(""),
        risk_moderator: Cow::Borrowed(""),
        fund_manager: Cow::Borrowed(
            "Synthetic test fund manager for {ticker} on {current_date}. Approve or reject.",
        ),
    };
    AnalysisPackManifest {
        // Reusing PackId::Baseline as a stand-in identifier — the manifest is
        // never registered or resolved through the registry; we only need a
        // valid PackId for the struct field. The synthetic content is what
        // distinguishes this manifest from baseline.
        id: PackId::Baseline,
        name: "Synthetic R8 Test Pack".to_owned(),
        description: "Test fixture for second-consumer API-shape contract assertions.".to_owned(),
        required_inputs: vec!["news".to_owned()],
        enrichment_intent: EnrichmentIntent {
            transcripts: false,
            consensus_estimates: false,
            event_news: false,
        },
        strategy_focus: StrategyFocus::Balanced,
        analysis_emphasis: "Synthetic emphasis (test fixture).".to_owned(),
        report_strategy_label: "synthetic-r8".to_owned(),
        default_valuation: ValuationAssessment::NotAssessed,
        prompt_bundle: bundle,
        valuator_selection: HashMap::new(),
    }
}

/// Build a synthetic manifest where one of the required slots is blanked
/// so completeness validation must reject it.
fn synthetic_one_role_manifest_with_blank_trader() -> AnalysisPackManifest {
    let mut manifest = synthetic_one_role_manifest();
    manifest.prompt_bundle.trader = Cow::Borrowed("");
    manifest
}

fn resolve_policy(manifest: &AnalysisPackManifest) -> scorpio_core::analysis_packs::RuntimePolicy {
    runtime_policy_from_manifest(manifest)
}

#[test]
fn synthetic_manifest_passes_completeness_under_zero_round_topology() {
    let manifest = synthetic_one_role_manifest();
    let policy = resolve_policy(&manifest);
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    let result = validate_active_pack_completeness(&policy, &topology);
    assert!(
        result.is_ok(),
        "synthetic one-role manifest should be complete under zero-rounds topology: {result:?}"
    );
}

#[test]
fn synthetic_manifest_with_blank_required_slot_fails_completeness() {
    let manifest = synthetic_one_role_manifest_with_blank_trader();
    let policy = resolve_policy(&manifest);
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    let err = validate_active_pack_completeness(&policy, &topology)
        .expect_err("blanking the trader slot must fail completeness");
    assert_eq!(err.missing_slots, vec![PromptSlot::Trader]);
}

#[test]
fn required_prompt_slots_derives_three_slots_for_one_role_zero_rounds() {
    // News + trader + fund_manager. No researchers (debate disabled), no
    // risk agents (risk disabled). Exactly the subset a one-role
    // synthetic-fixture manifest needs.
    let manifest = synthetic_one_role_manifest();
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    let slots = required_prompt_slots(&topology);
    assert_eq!(slots.len(), 3, "expected three slots, got {slots:?}");
    assert!(slots.contains(&PromptSlot::NewsAnalyst));
    assert!(slots.contains(&PromptSlot::Trader));
    assert!(slots.contains(&PromptSlot::FundManager));
}

#[test]
fn required_prompt_slots_omits_baseline_specific_slots_under_one_role_roster() {
    // Verify that the four equity-only analyst slots NOT in the synthetic
    // roster are correctly omitted — this is the load-bearing claim that
    // proves required_prompt_slots subsets correctly for a non-baseline
    // analyst roster.
    let manifest = synthetic_one_role_manifest();
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    let slots = required_prompt_slots(&topology);
    assert!(!slots.contains(&PromptSlot::FundamentalAnalyst));
    assert!(!slots.contains(&PromptSlot::SentimentAnalyst));
    assert!(!slots.contains(&PromptSlot::TechnicalAnalyst));
}

#[test]
fn synthetic_manifest_completeness_scales_with_topology_enable_flags() {
    // Same one-role roster but with both stages enabled — completeness
    // should *fail* because the synthetic bundle does not populate
    // researcher / risk / moderator slots. This proves the topology's
    // stage-enable flags actually drive the required-slot set.
    let manifest = synthetic_one_role_manifest();
    let policy = resolve_policy(&manifest);
    let full_topology = build_run_topology(&manifest.required_inputs, 1, 1);
    let err = validate_active_pack_completeness(&policy, &full_topology)
        .expect_err("fully-enabled topology over a partial bundle must fail completeness");
    // Researchers + debate moderator + 3 risk agents + risk moderator = 7
    // additional required slots beyond the zero-rounds case.
    assert_eq!(
        err.missing_slots.len(),
        7,
        "expected 7 missing slots (debate + risk slots), got {:?}",
        err.missing_slots
    );
}

#[test]
fn one_role_topology_tracks_only_the_declared_spawned_analyst() {
    let manifest = synthetic_one_role_manifest();
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    assert_eq!(topology.spawned_analysts.len(), 1);
    assert!(topology.spawned_analysts.contains(&Role::NewsAnalyst));
    assert!(topology.unknown_inputs.is_empty());
}

#[test]
fn synthetic_manifest_fails_closed_when_required_inputs_include_unknown_entry() {
    let mut manifest = synthetic_one_role_manifest();
    manifest.required_inputs.push("tokenomics".to_owned());
    let policy = resolve_policy(&manifest);
    let topology = build_run_topology(&manifest.required_inputs, 0, 0);
    let err = validate_active_pack_completeness(&policy, &topology)
        .expect_err("unknown required_inputs must fail this API-shape contract test");
    assert_eq!(err.missing_slots, Vec::<PromptSlot>::new());
    assert_eq!(err.unknown_inputs, vec!["tokenomics".to_owned()]);
}

#[test]
fn synthetic_manifest_uses_distinct_prose_from_baseline() {
    // Sanity: confirm the fixture is not accidentally aliased to baseline
    // content. If the fixture were to copy baseline strings, this test
    // would fail and the maintainer would be forced to write fresh prose.
    let synthetic = synthetic_one_role_manifest();
    let baseline = scorpio_core::analysis_packs::resolve_pack(PackId::Baseline);
    assert_ne!(
        synthetic.prompt_bundle.news_analyst, baseline.prompt_bundle.news_analyst,
        "synthetic news_analyst must differ from baseline so the abstraction is exercised \
         against genuinely non-baseline content"
    );
}
