use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::state::{TradeAction, TradeProposal};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TraderProposalResponse {
    pub action: TradeAction,
    pub target_price: f64,
    pub stop_loss: f64,
    pub confidence: f64,
    pub rationale: String,
    #[serde(default)]
    pub valuation_assessment: Option<String>,
}

impl From<TraderProposalResponse> for TradeProposal {
    fn from(value: TraderProposalResponse) -> Self {
        Self {
            action: value.action,
            target_price: value.target_price,
            stop_loss: value.stop_loss,
            confidence: value.confidence,
            rationale: value.rationale,
            valuation_assessment: value.valuation_assessment,
            scenario_valuation: None,
        }
    }
}
