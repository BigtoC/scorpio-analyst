use serde::{Deserialize, Serialize};

use super::TradeAction;

/// Final decision issued by the Fund Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Approved,
    Rejected,
}

/// Terminal execution status for a trading cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStatus {
    pub decision: Decision,
    pub action: TradeAction,
    pub rationale: String,
    pub decided_at: String,
    /// Action-conditional entry guidance. For Buy/Overweight/Hold this is a laddered
    /// tier plan (e.g. "Tier 1 (40%) on dip to $530-535 …; cancel below $490"); for
    /// Underweight/Sell this is a re-entry condition (price level or thesis-change
    /// criterion). Required for every action and enforced by `validate_execution_status`.
    #[serde(default)]
    pub entry_guidance: Option<String>,
    /// Suggested position sizing, e.g. "5–12% of portfolio (add 2–4% on weakness)".
    #[serde(default)]
    pub suggested_position: Option<String>,
}
