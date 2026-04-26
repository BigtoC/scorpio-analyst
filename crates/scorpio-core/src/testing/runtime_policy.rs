//! Test-only helpers for hydrating runtime policy without traversing
//! `PreflightTask`.
//!
//! These helpers are gated behind `#[cfg(any(test, feature = "test-helpers"))]`
//! and exist so unit/integration tests that exercise prompt builders, agent
//! tasks, or graph fragments in isolation can populate the runtime policy
//! their consumers expect. **Production code must never use these — preflight
//! is the sole writer of `state.analysis_runtime_policy` per Unit 4a.**

use std::sync::OnceLock;

use crate::analysis_packs::{
    AnalysisPackManifest, PackId, RuntimePolicy, resolve_pack, resolve_runtime_policy_for_manifest,
};
use crate::state::TradingState;
use crate::workflow::Role;

/// Cached baseline manifest used by [`baseline_pack_prompt_for_role`] so its
/// `Cow::Owned` slot data lives for the lifetime of the test process — that
/// lets the oracle keep returning `&'static str` even after the equity pack
/// started materialising owned slot content (analyst slots gain the runtime
/// contract via `with_analyst_runtime_contract` at load time).
fn baseline_manifest() -> &'static AnalysisPackManifest {
    static MANIFEST: OnceLock<AnalysisPackManifest> = OnceLock::new();
    MANIFEST.get_or_init(|| resolve_pack(PackId::Baseline))
}

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
///
/// Backed by a `OnceLock`-cached manifest so analyst slots that materialise
/// owned strings at load time (the runtime contract is appended in
/// `baseline_prompt_bundle`) still return `&'static str` to callers.
#[must_use]
pub fn baseline_pack_prompt_for_role(role: Role) -> &'static str {
    use crate::workflow::PromptSlot;

    let bundle = &baseline_manifest().prompt_bundle;
    match role.prompt_slot() {
        PromptSlot::FundamentalAnalyst => bundle.fundamental_analyst.as_ref(),
        PromptSlot::SentimentAnalyst => bundle.sentiment_analyst.as_ref(),
        PromptSlot::NewsAnalyst => bundle.news_analyst.as_ref(),
        PromptSlot::TechnicalAnalyst => bundle.technical_analyst.as_ref(),
        PromptSlot::BullishResearcher => bundle.bullish_researcher.as_ref(),
        PromptSlot::BearishResearcher => bundle.bearish_researcher.as_ref(),
        PromptSlot::DebateModerator => bundle.debate_moderator.as_ref(),
        PromptSlot::Trader => bundle.trader.as_ref(),
        PromptSlot::AggressiveRisk => bundle.aggressive_risk.as_ref(),
        PromptSlot::ConservativeRisk => bundle.conservative_risk.as_ref(),
        PromptSlot::NeutralRisk => bundle.neutral_risk.as_ref(),
        PromptSlot::RiskModerator => bundle.risk_moderator.as_ref(),
        PromptSlot::FundManager => bundle.fund_manager.as_ref(),
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
    fn baseline_prompt_matches_bundle_field_for_every_role() {
        // Pack-oracle helper must return byte-identical content to the
        // manifest's PromptBundle for every live role — locks the helper as
        // the canonical regression-test oracle for Units 4a/4b.
        let bundle = &baseline_manifest().prompt_bundle;
        let pairs: [(Role, &str); 13] = [
            (
                Role::FundamentalAnalyst,
                bundle.fundamental_analyst.as_ref(),
            ),
            (Role::SentimentAnalyst, bundle.sentiment_analyst.as_ref()),
            (Role::NewsAnalyst, bundle.news_analyst.as_ref()),
            (Role::TechnicalAnalyst, bundle.technical_analyst.as_ref()),
            (Role::BullishResearcher, bundle.bullish_researcher.as_ref()),
            (Role::BearishResearcher, bundle.bearish_researcher.as_ref()),
            (Role::DebateModerator, bundle.debate_moderator.as_ref()),
            (Role::Trader, bundle.trader.as_ref()),
            (Role::AggressiveRisk, bundle.aggressive_risk.as_ref()),
            (Role::ConservativeRisk, bundle.conservative_risk.as_ref()),
            (Role::NeutralRisk, bundle.neutral_risk.as_ref()),
            (Role::RiskModerator, bundle.risk_moderator.as_ref()),
            (Role::FundManager, bundle.fund_manager.as_ref()),
        ];
        for (role, expected) in pairs {
            assert_eq!(
                baseline_pack_prompt_for_role(role),
                expected,
                "oracle for {role:?} must match manifest bundle byte-for-byte"
            );
        }
    }
}
