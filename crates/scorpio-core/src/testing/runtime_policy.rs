//! Test-only helpers for hydrating runtime policy without traversing
//! `PreflightTask`.
//!
//! These helpers are gated behind `#[cfg(any(test, feature = "test-helpers"))]`
//! and exist so unit/integration tests that exercise prompt builders, agent
//! tasks, or graph fragments in isolation can populate the runtime policy
//! their consumers expect. **Production code must never use these — preflight
//! is the sole writer of `state.analysis_runtime_policy` per Unit 4a.**

use crate::analysis_packs::{
    PackId, RuntimePolicy, resolve_pack, resolve_runtime_policy_for_manifest,
};
use crate::prompts::PromptBundle;
use crate::state::TradingState;
use crate::workflow::topology::Role;

/// Hydrate `state.analysis_runtime_policy` with the baseline pack's
/// `RuntimePolicy`. Idempotent.
///
/// Callers that need a non-baseline policy can use [`with_runtime_policy`]
/// directly, or build a synthetic manifest and resolve a policy from it.
pub fn with_baseline_runtime_policy(state: &mut TradingState) {
    let manifest = resolve_pack(PackId::Baseline);
    let policy = resolve_runtime_policy_for_manifest(&manifest)
        .expect("baseline manifest must resolve to a runtime policy");
    state.analysis_runtime_policy = Some(policy);
}

/// Hydrate `state.analysis_runtime_policy` with the supplied `policy`,
/// replacing any previous value. Useful when tests need a non-baseline
/// or hand-rolled policy.
pub fn with_runtime_policy(state: &mut TradingState, policy: RuntimePolicy) {
    state.analysis_runtime_policy = Some(policy);
}

/// Borrow the baseline pack's prompt-asset text for a given role.
///
/// Mirrors how the production runtime reads a slot off the active pack's
/// `PromptBundle` after preflight has hydrated it. Tests use this as the
/// canonical oracle for "what does the baseline pack say for role X" without
/// having to rebuild a runtime policy.
#[must_use]
pub fn baseline_pack_prompt_for_role(role: Role) -> &'static str {
    let manifest = resolve_pack(PackId::Baseline);
    // The baseline pack uses `Cow::Borrowed(include_str!(...))` so each slot's
    // backing storage is `&'static str`. We round-trip through the slot
    // accessor to keep this helper consistent with how production code reads
    // the bundle, but the lifetime is preserved by extracting the static
    // borrow up front.
    let bundle: PromptBundle = manifest.prompt_bundle.clone();
    let slot = role.prompt_slot();
    // Match every slot explicitly so a future PromptSlot variant forces a
    // compile error here too.
    use crate::workflow::topology::PromptSlot;
    let cow = match slot {
        PromptSlot::FundamentalAnalyst => bundle.fundamental_analyst,
        PromptSlot::SentimentAnalyst => bundle.sentiment_analyst,
        PromptSlot::NewsAnalyst => bundle.news_analyst,
        PromptSlot::TechnicalAnalyst => bundle.technical_analyst,
        PromptSlot::BullishResearcher => bundle.bullish_researcher,
        PromptSlot::BearishResearcher => bundle.bearish_researcher,
        PromptSlot::DebateModerator => bundle.debate_moderator,
        PromptSlot::Trader => bundle.trader,
        PromptSlot::AggressiveRisk => bundle.aggressive_risk,
        PromptSlot::ConservativeRisk => bundle.conservative_risk,
        PromptSlot::NeutralRisk => bundle.neutral_risk,
        PromptSlot::RiskModerator => bundle.risk_moderator,
        PromptSlot::FundManager => bundle.fund_manager,
    };
    match cow {
        std::borrow::Cow::Borrowed(s) => s,
        // Baseline assets are `include_str!` and therefore always Borrowed;
        // a runtime-loaded pack would hit this branch and need a different
        // helper.
        std::borrow::Cow::Owned(_) => {
            panic!("baseline pack prompts must be compile-time borrowed (include_str!)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_baseline_runtime_policy_hydrates_state() {
        let mut state = TradingState::new("AAPL".to_owned(), "2026-04-25".to_owned());
        assert!(state.analysis_runtime_policy.is_none());
        with_baseline_runtime_policy(&mut state);
        let policy = state
            .analysis_runtime_policy
            .as_ref()
            .expect("policy hydrated");
        assert_eq!(policy.pack_id, PackId::Baseline);
        assert!(!policy.prompt_bundle.is_empty());
    }

    #[test]
    fn with_baseline_runtime_policy_is_idempotent() {
        let mut state = TradingState::new("MSFT".to_owned(), "2026-04-25".to_owned());
        with_baseline_runtime_policy(&mut state);
        with_baseline_runtime_policy(&mut state);
        assert!(state.analysis_runtime_policy.is_some());
    }

    #[test]
    fn baseline_prompt_for_each_live_role_is_non_empty() {
        for role in [
            Role::FundamentalAnalyst,
            Role::SentimentAnalyst,
            Role::NewsAnalyst,
            Role::TechnicalAnalyst,
            Role::BullishResearcher,
            Role::BearishResearcher,
            Role::DebateModerator,
            Role::Trader,
            Role::AggressiveRisk,
            Role::ConservativeRisk,
            Role::NeutralRisk,
            Role::RiskModerator,
            Role::FundManager,
        ] {
            let prompt = baseline_pack_prompt_for_role(role);
            assert!(
                !prompt.trim().is_empty(),
                "baseline prompt for {role:?} must be non-empty"
            );
        }
    }

    #[test]
    fn baseline_prompt_matches_bundle_field_for_role() {
        let manifest = resolve_pack(PackId::Baseline);
        assert_eq!(
            baseline_pack_prompt_for_role(Role::FundamentalAnalyst),
            manifest.prompt_bundle.fundamental_analyst.as_ref()
        );
        assert_eq!(
            baseline_pack_prompt_for_role(Role::FundManager),
            manifest.prompt_bundle.fund_manager.as_ref()
        );
    }
}
