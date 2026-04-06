pub(super) struct TaskIds {
    pub preflight: &'static str,
    pub analyst_fan_out: &'static str,
    pub analyst_sync: &'static str,
    pub bullish_researcher: &'static str,
    pub bearish_researcher: &'static str,
    pub debate_moderator: &'static str,
    pub trader: &'static str,
    pub aggressive_risk: &'static str,
    pub conservative_risk: &'static str,
    pub neutral_risk: &'static str,
    pub risk_moderator: &'static str,
    pub fund_manager: &'static str,
}

pub(super) const TASKS: TaskIds = TaskIds {
    preflight: "preflight",
    analyst_fan_out: "analyst_fanout",
    analyst_sync: "analyst_sync",
    bullish_researcher: "bullish_researcher",
    bearish_researcher: "bearish_researcher",
    debate_moderator: "debate_moderator",
    trader: "trader",
    aggressive_risk: "aggressive_risk",
    conservative_risk: "conservative_risk",
    neutral_risk: "neutral_risk",
    risk_moderator: "risk_moderator",
    fund_manager: "fund_manager",
};

#[cfg(any(test, feature = "test-helpers"))]
pub(super) const REPLACEABLE_TASK_IDS: [&str; 12] = [
    TASKS.preflight,
    TASKS.analyst_fan_out,
    TASKS.analyst_sync,
    TASKS.bullish_researcher,
    TASKS.bearish_researcher,
    TASKS.debate_moderator,
    TASKS.trader,
    TASKS.aggressive_risk,
    TASKS.conservative_risk,
    TASKS.neutral_risk,
    TASKS.risk_moderator,
    TASKS.fund_manager,
];
