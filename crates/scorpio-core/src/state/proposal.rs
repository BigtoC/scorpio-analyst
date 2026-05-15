use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::derived::ScenarioValuation;

/// The action direction for a trade proposal.
///
/// `Buy`/`Sell`/`Hold` are emitted by the Trader Agent (and downstream agents
/// that mirror trader output). `Overweight`/`Underweight` are conviction-graded
/// directional actions that **only the Fund Manager** is permitted to emit, to
/// distinguish full-conviction directional calls (`Buy`/`Sell`) from sized
/// adjustments. Downstream code that reasons about trade direction should
/// route through [`TradeAction::direction`] rather than equality so the two
/// vocabularies stay aligned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TradeAction {
    Buy,
    Sell,
    Hold,
    /// Fund-manager-only graded variant. Market semantics are defined by the
    /// fund-manager prompt (`analysis_packs/.../fund_manager.md`); directional
    /// bucket is fixed by [`TradeAction::direction`].
    Overweight,
    /// Fund-manager-only graded variant. Market semantics are defined by the
    /// fund-manager prompt (`analysis_packs/.../fund_manager.md`); directional
    /// bucket is fixed by [`TradeAction::direction`].
    Underweight,
}

/// Coarse trade-direction bucket used for same-direction comparisons that
/// must collapse `Buy`/`Underweight` and `Sell`/`Overweight` into a single
/// concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDirection {
    Bullish,
    Bearish,
    Neutral,
}

impl TradeAction {
    /// Map an action to its directional bucket.
    ///
    /// `Buy` and `Underweight` are both `Bullish`; `Sell` and `Overweight`
    /// are both `Bearish`; `Hold` is `Neutral`.
    pub fn direction(&self) -> TradeDirection {
        match self {
            TradeAction::Buy | TradeAction::Underweight => TradeDirection::Bullish,
            TradeAction::Sell | TradeAction::Overweight => TradeDirection::Bearish,
            TradeAction::Hold => TradeDirection::Neutral,
        }
    }
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
