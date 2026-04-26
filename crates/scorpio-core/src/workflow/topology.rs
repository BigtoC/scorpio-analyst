//! Per-run topology — the single source of truth for which agent roles run,
//! which prompt slots are required, and how zero-round routing flips.
//!
//! `PreflightTask` builds a `RunRoleTopology` from the active pack manifest
//! and the configured round counts. Downstream consumers — analyst fan-out
//! (via `RoutingFlags` per-child gating), `validate_active_pack_completeness`
//! (via `required_prompt_slots`), and the conditional-edge closures in
//! `workflow::builder` (via `RoutingFlags`) — all read derived views of the
//! same topology so they cannot drift.
//!
//! The role-to-slot mapping is encoded as an exhaustive `match` over `Role`
//! with no wildcard arm: adding a new `Role` variant becomes a compile error
//! until `Role::prompt_slot` is extended.

use std::collections::BTreeSet;

use crate::prompts::PromptBundle;

/// Every agent role that can participate in a pipeline run.
///
/// One variant per slot in [`PromptBundle`], in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    FundamentalAnalyst,
    SentimentAnalyst,
    NewsAnalyst,
    TechnicalAnalyst,
    BullishResearcher,
    BearishResearcher,
    DebateModerator,
    Trader,
    AggressiveRisk,
    ConservativeRisk,
    NeutralRisk,
    RiskModerator,
    FundManager,
}

/// Identifier for a slot inside a [`PromptBundle`].
///
/// Used by `required_prompt_slots` and `validate_active_pack_completeness`
/// so callers can address slots without holding a `&PromptBundle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptSlot {
    FundamentalAnalyst,
    SentimentAnalyst,
    NewsAnalyst,
    TechnicalAnalyst,
    BullishResearcher,
    BearishResearcher,
    DebateModerator,
    Trader,
    AggressiveRisk,
    ConservativeRisk,
    NeutralRisk,
    RiskModerator,
    FundManager,
}

impl Role {
    /// Map a `Role` to the prompt slot it reads at render time.
    ///
    /// Encoded as an exhaustive `match` with no wildcard arm so any future
    /// `Role` addition becomes a compile error until this table is extended.
    #[must_use]
    pub fn prompt_slot(self) -> PromptSlot {
        match self {
            Role::FundamentalAnalyst => PromptSlot::FundamentalAnalyst,
            Role::SentimentAnalyst => PromptSlot::SentimentAnalyst,
            Role::NewsAnalyst => PromptSlot::NewsAnalyst,
            Role::TechnicalAnalyst => PromptSlot::TechnicalAnalyst,
            Role::BullishResearcher => PromptSlot::BullishResearcher,
            Role::BearishResearcher => PromptSlot::BearishResearcher,
            Role::DebateModerator => PromptSlot::DebateModerator,
            Role::Trader => PromptSlot::Trader,
            Role::AggressiveRisk => PromptSlot::AggressiveRisk,
            Role::ConservativeRisk => PromptSlot::ConservativeRisk,
            Role::NeutralRisk => PromptSlot::NeutralRisk,
            Role::RiskModerator => PromptSlot::RiskModerator,
            Role::FundManager => PromptSlot::FundManager,
        }
    }

    /// True iff this role is one of the four analyst roles that participate
    /// in the parallel fan-out at the start of a run.
    ///
    /// Exhaustive match defends against silent omission when a new `Role` is
    /// added: the compiler forces the maintainer to classify it.
    #[must_use]
    pub fn is_analyst(self) -> bool {
        match self {
            Role::FundamentalAnalyst
            | Role::SentimentAnalyst
            | Role::NewsAnalyst
            | Role::TechnicalAnalyst => true,
            Role::BullishResearcher
            | Role::BearishResearcher
            | Role::DebateModerator
            | Role::Trader
            | Role::AggressiveRisk
            | Role::ConservativeRisk
            | Role::NeutralRisk
            | Role::RiskModerator
            | Role::FundManager => false,
        }
    }
}

impl PromptSlot {
    /// Borrow the slot value from a [`PromptBundle`].
    ///
    /// Exhaustive `match` keeps the bundle layout and slot identifiers in
    /// lockstep: renaming a `PromptBundle` field forces this match to update.
    #[must_use]
    pub fn read(self, bundle: &PromptBundle) -> &str {
        match self {
            PromptSlot::FundamentalAnalyst => &bundle.fundamental_analyst,
            PromptSlot::SentimentAnalyst => &bundle.sentiment_analyst,
            PromptSlot::NewsAnalyst => &bundle.news_analyst,
            PromptSlot::TechnicalAnalyst => &bundle.technical_analyst,
            PromptSlot::BullishResearcher => &bundle.bullish_researcher,
            PromptSlot::BearishResearcher => &bundle.bearish_researcher,
            PromptSlot::DebateModerator => &bundle.debate_moderator,
            PromptSlot::Trader => &bundle.trader,
            PromptSlot::AggressiveRisk => &bundle.aggressive_risk,
            PromptSlot::ConservativeRisk => &bundle.conservative_risk,
            PromptSlot::NeutralRisk => &bundle.neutral_risk,
            PromptSlot::RiskModerator => &bundle.risk_moderator,
            PromptSlot::FundManager => &bundle.fund_manager,
        }
    }

    /// Stable, machine-readable name for diagnostics and stable error ordering.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PromptSlot::FundamentalAnalyst => "fundamental_analyst",
            PromptSlot::SentimentAnalyst => "sentiment_analyst",
            PromptSlot::NewsAnalyst => "news_analyst",
            PromptSlot::TechnicalAnalyst => "technical_analyst",
            PromptSlot::BullishResearcher => "bullish_researcher",
            PromptSlot::BearishResearcher => "bearish_researcher",
            PromptSlot::DebateModerator => "debate_moderator",
            PromptSlot::Trader => "trader",
            PromptSlot::AggressiveRisk => "aggressive_risk",
            PromptSlot::ConservativeRisk => "conservative_risk",
            PromptSlot::NeutralRisk => "neutral_risk",
            PromptSlot::RiskModerator => "risk_moderator",
            PromptSlot::FundManager => "fund_manager",
        }
    }
}

/// The set of roles that will actually run in a single pipeline cycle, plus
/// stage-level enable flags derived from the configured round counts.
///
/// `spawned_analysts` is the maximal set of analysts that the fan-out builds;
/// individual analysts no-op at run time when their role is not in this set.
/// `debate_enabled` and `risk_enabled` map directly to the corresponding
/// [`RoutingFlags`] but are stored on the topology so all derivations
/// (`required_prompt_slots`, fan-out gating, conditional-edge closures) read
/// the same source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRoleTopology {
    pub spawned_analysts: BTreeSet<Role>,
    pub unknown_inputs: Vec<String>,
    pub debate_enabled: bool,
    pub risk_enabled: bool,
}

/// Per-run routing decisions written into `Context` for graph-flow
/// conditional-edge closures and per-child analyst gating.
///
/// Loop-back conditionals (`round < max`) continue to use the existing
/// per-iteration counters; `RoutingFlags` only governs *entry* into the
/// debate and risk stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingFlags {
    pub skip_debate: bool,
    pub skip_risk: bool,
}

impl RoutingFlags {
    /// Derive the runtime routing flags from a topology.
    ///
    /// Any future flag follows the same one-source pattern: read from the
    /// topology, never compute independently.
    #[must_use]
    pub fn from_topology(topology: &RunRoleTopology) -> Self {
        Self {
            skip_debate: !topology.debate_enabled,
            skip_risk: !topology.risk_enabled,
        }
    }
}

/// Map a manifest `required_inputs` string to the analyst role that consumes
/// it. Returns `None` for inputs the current `Role` enum does not yet model
/// (e.g., crypto-only inputs while the crypto pack is still inactive).
///
/// The mapping is intentionally narrow: only the four equity inputs map
/// today. New asset classes extend the `Role` enum first, then add arms here.
#[must_use]
pub fn analyst_role_for_input(input: &str) -> Option<Role> {
    match input {
        "fundamentals" => Some(Role::FundamentalAnalyst),
        "sentiment" => Some(Role::SentimentAnalyst),
        "news" => Some(Role::NewsAnalyst),
        "technical" => Some(Role::TechnicalAnalyst),
        _ => None,
    }
}

/// Build a per-run topology from manifest inputs and configured round counts.
///
/// `max_debate_rounds == 0` means the debate stage is bypassed entirely (no
/// researchers, no moderator). `max_risk_rounds == 0` does the same for the
/// risk stage. The trader and fund manager are always part of the run.
#[must_use]
pub fn build_run_topology(
    required_inputs: &[String],
    max_debate_rounds: u32,
    max_risk_rounds: u32,
) -> RunRoleTopology {
    let mut spawned_analysts: BTreeSet<Role> = BTreeSet::new();
    let mut unknown_inputs: Vec<String> = Vec::new();

    for input in required_inputs {
        match analyst_role_for_input(input) {
            Some(role) => {
                spawned_analysts.insert(role);
            }
            None => unknown_inputs.push(input.clone()),
        }
    }

    RunRoleTopology {
        spawned_analysts,
        unknown_inputs,
        debate_enabled: max_debate_rounds > 0,
        risk_enabled: max_risk_rounds > 0,
    }
}

/// The slots a pack manifest must populate for the given topology.
///
/// Trader and FundManager slots are always required. Analyst slots come from
/// `topology.spawned_analysts`. Researcher / debate-moderator slots only when
/// debate is enabled; risk-agent / risk-moderator slots only when risk is
/// enabled. The returned `BTreeSet` iterates in stable order for
/// deterministic multi-slot diagnostics.
#[must_use]
pub fn required_prompt_slots(topology: &RunRoleTopology) -> BTreeSet<PromptSlot> {
    let mut slots: BTreeSet<PromptSlot> = topology
        .spawned_analysts
        .iter()
        .map(|role| role.prompt_slot())
        .collect();

    slots.insert(PromptSlot::Trader);
    slots.insert(PromptSlot::FundManager);

    if topology.debate_enabled {
        slots.insert(PromptSlot::BullishResearcher);
        slots.insert(PromptSlot::BearishResearcher);
        slots.insert(PromptSlot::DebateModerator);
    }

    if topology.risk_enabled {
        slots.insert(PromptSlot::AggressiveRisk);
        slots.insert(PromptSlot::ConservativeRisk);
        slots.insert(PromptSlot::NeutralRisk);
        slots.insert(PromptSlot::RiskModerator);
    }

    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equity_inputs() -> Vec<String> {
        vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ]
    }

    #[test]
    fn baseline_topology_has_four_analysts_plus_debate_and_risk() {
        let topology = build_run_topology(&equity_inputs(), 2, 2);
        assert_eq!(topology.spawned_analysts.len(), 4);
        assert!(topology.unknown_inputs.is_empty());
        assert!(
            topology
                .spawned_analysts
                .contains(&Role::FundamentalAnalyst)
        );
        assert!(topology.spawned_analysts.contains(&Role::SentimentAnalyst));
        assert!(topology.spawned_analysts.contains(&Role::NewsAnalyst));
        assert!(topology.spawned_analysts.contains(&Role::TechnicalAnalyst));
        assert!(topology.debate_enabled);
        assert!(topology.risk_enabled);
    }

    #[test]
    fn baseline_topology_requires_thirteen_slots() {
        let topology = build_run_topology(&equity_inputs(), 2, 2);
        let slots = required_prompt_slots(&topology);
        assert_eq!(slots.len(), 13, "fully-enabled baseline requires 13 slots");
    }

    #[test]
    fn zero_debate_rounds_omits_researcher_and_debate_moderator() {
        let topology = build_run_topology(&equity_inputs(), 0, 2);
        assert!(!topology.debate_enabled);
        let slots = required_prompt_slots(&topology);
        assert!(!slots.contains(&PromptSlot::BullishResearcher));
        assert!(!slots.contains(&PromptSlot::BearishResearcher));
        assert!(!slots.contains(&PromptSlot::DebateModerator));
        // Risk + analysts + trader + fund_manager still required.
        assert!(slots.contains(&PromptSlot::AggressiveRisk));
        assert!(slots.contains(&PromptSlot::Trader));
        assert!(slots.contains(&PromptSlot::FundManager));
    }

    #[test]
    fn zero_risk_rounds_omits_risk_agents_and_moderator() {
        let topology = build_run_topology(&equity_inputs(), 2, 0);
        assert!(!topology.risk_enabled);
        let slots = required_prompt_slots(&topology);
        assert!(!slots.contains(&PromptSlot::AggressiveRisk));
        assert!(!slots.contains(&PromptSlot::ConservativeRisk));
        assert!(!slots.contains(&PromptSlot::NeutralRisk));
        assert!(!slots.contains(&PromptSlot::RiskModerator));
        // Debate stage and trader/fund_manager remain.
        assert!(slots.contains(&PromptSlot::BullishResearcher));
        assert!(slots.contains(&PromptSlot::Trader));
        assert!(slots.contains(&PromptSlot::FundManager));
    }

    #[test]
    fn zero_both_rounds_keeps_only_analysts_trader_fund_manager() {
        let topology = build_run_topology(&equity_inputs(), 0, 0);
        let slots = required_prompt_slots(&topology);
        // 4 analysts + trader + fund_manager = 6 slots.
        assert_eq!(slots.len(), 6);
        assert!(slots.contains(&PromptSlot::Trader));
        assert!(slots.contains(&PromptSlot::FundManager));
        assert!(slots.contains(&PromptSlot::FundamentalAnalyst));
    }

    #[test]
    fn unknown_inputs_are_tracked_fail_closed() {
        // Crypto-only inputs do not yet have Role variants. Topology still
        // records them so completeness/diagnostics can fail closed instead of
        // pretending the pack has an empty-but-valid analyst roster.
        let inputs = vec![
            "tokenomics".to_owned(),
            "onchain".to_owned(),
            "social".to_owned(),
            "derivatives".to_owned(),
        ];
        let topology = build_run_topology(&inputs, 2, 2);
        assert!(topology.spawned_analysts.is_empty());
        assert_eq!(
            topology.unknown_inputs,
            vec![
                "tokenomics".to_owned(),
                "onchain".to_owned(),
                "social".to_owned(),
                "derivatives".to_owned(),
            ]
        );
    }

    #[test]
    fn one_role_roster_is_supported() {
        // Synthetic non-baseline pack with a single analyst role — the
        // R8 abstraction-test fixture shape.
        let inputs = vec!["news".to_owned()];
        let topology = build_run_topology(&inputs, 0, 0);
        let slots = required_prompt_slots(&topology);
        assert_eq!(slots.len(), 3); // news + trader + fund_manager
        assert!(slots.contains(&PromptSlot::NewsAnalyst));
        assert!(slots.contains(&PromptSlot::Trader));
        assert!(slots.contains(&PromptSlot::FundManager));
    }

    #[test]
    fn role_prompt_slot_is_one_to_one() {
        // Every role maps to exactly its name-matching slot.
        assert_eq!(
            Role::FundamentalAnalyst.prompt_slot(),
            PromptSlot::FundamentalAnalyst
        );
        assert_eq!(Role::Trader.prompt_slot(), PromptSlot::Trader);
        assert_eq!(Role::FundManager.prompt_slot(), PromptSlot::FundManager);
    }

    #[test]
    fn is_analyst_classifies_four_roles_only() {
        assert!(Role::FundamentalAnalyst.is_analyst());
        assert!(Role::SentimentAnalyst.is_analyst());
        assert!(Role::NewsAnalyst.is_analyst());
        assert!(Role::TechnicalAnalyst.is_analyst());
        assert!(!Role::BullishResearcher.is_analyst());
        assert!(!Role::Trader.is_analyst());
        assert!(!Role::FundManager.is_analyst());
        assert!(!Role::AggressiveRisk.is_analyst());
        assert!(!Role::RiskModerator.is_analyst());
    }

    #[test]
    fn routing_flags_invert_topology_enables() {
        let zero_both = build_run_topology(&equity_inputs(), 0, 0);
        let flags = RoutingFlags::from_topology(&zero_both);
        assert!(flags.skip_debate);
        assert!(flags.skip_risk);

        let full = build_run_topology(&equity_inputs(), 2, 2);
        let flags = RoutingFlags::from_topology(&full);
        assert!(!flags.skip_debate);
        assert!(!flags.skip_risk);
    }

    #[test]
    fn slot_read_returns_bundle_field() {
        let bundle = PromptBundle::from_static(
            "F", "S", "N", "T", "Bull", "Bear", "DM", "Tr", "Ag", "Co", "Ne", "RM", "FM",
        );
        assert_eq!(PromptSlot::FundamentalAnalyst.read(&bundle), "F");
        assert_eq!(PromptSlot::SentimentAnalyst.read(&bundle), "S");
        assert_eq!(PromptSlot::FundManager.read(&bundle), "FM");
    }

    #[test]
    fn slot_name_is_stable_and_snake_case() {
        // Names feed multi-slot diagnostic ordering; assert they are stable
        // strings so any rename forces a deliberate test update.
        assert_eq!(PromptSlot::FundamentalAnalyst.name(), "fundamental_analyst");
        assert_eq!(PromptSlot::DebateModerator.name(), "debate_moderator");
        assert_eq!(PromptSlot::FundManager.name(), "fund_manager");
    }
}
