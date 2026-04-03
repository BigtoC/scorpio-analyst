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
    /// Tactical entry guidance, e.g. "BUY on any dip below $570–$575".
    /// Required when the recommendation is Hold or Sell.
    #[serde(default)]
    pub entry_guidance: Option<String>,
    /// Suggested position sizing, e.g. "5–12% of portfolio (add 2–4% on weakness)".
    #[serde(default)]
    pub suggested_position: Option<String>,
}
