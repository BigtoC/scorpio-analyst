use serde::{Deserialize, Serialize};

/// Risk tolerance level of a risk agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Aggressive,
    Neutral,
    Conservative,
}

/// Assessment produced by a risk-management agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskReport {
    pub risk_level: RiskLevel,
    pub assessment: String,
    pub recommended_adjustments: Vec<String>,
    pub flags_violation: bool,
}
