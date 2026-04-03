use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The action direction for a trade proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum TradeAction {
    Buy,
    Sell,
    Hold,
}

/// A structured trade proposal emitted by the Trader Agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TradeProposal {
    pub action: TradeAction,
    pub target_price: f64,
    pub stop_loss: f64,
    pub confidence: f64,
    pub rationale: String,
    /// Valuation assessment: "overvalued", "undervalued", or "fair value" with brief justification.
    #[serde(default)]
    pub valuation_assessment: Option<String>,
}
