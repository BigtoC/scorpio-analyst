use serde::{Deserialize, Serialize};

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
    pub rationale: String,
    pub decided_at: String,
}
