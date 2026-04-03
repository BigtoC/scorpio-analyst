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
}
