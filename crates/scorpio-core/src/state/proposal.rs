use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::derived::ScenarioValuation;

/// The action direction for a trade proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// Deterministic scenario valuation (DCF, EV/EBITDA, Forward P/E, PEG) computed before
    /// this proposal was generated.
    ///
    /// `None` for pre-feature snapshots or when valuation was not computed for this run.
    /// If valuation does not apply to this asset shape, the runtime stores
    /// `Some(ScenarioValuation::NotAssessed { .. })` instead.
    /// This field is runtime-owned and excluded from the LLM response schema;
    /// the runtime populates it after trader inference.
    #[schemars(
        description = "Deterministic scenario valuation computed before this proposal. None for pre-feature snapshots or when valuation was not computed for this run. For assets where valuation does not apply (e.g. ETF), the runtime stores a NotAssessed variant instead. This field is runtime-owned and excluded from the LLM response schema."
    )]
    #[serde(default)]
    pub scenario_valuation: Option<ScenarioValuation>,
}
