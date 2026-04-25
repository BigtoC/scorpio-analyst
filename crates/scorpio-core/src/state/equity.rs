//! Equity-scoped state — the ten fields that only make sense for listed
//! equity instruments.
//!
//! Phase 6 of the asset-class generalization refactor moves these fields
//! off [`super::TradingState`] root so crypto (and future classes) don't
//! have to leave them unset. Accessor methods on `TradingState` stay the
//! preferred call-site shape; raw field access via [`EquityState`] is
//! available when callers genuinely need to pattern-match on the whole
//! equity sub-state (e.g. snapshot serialization).
use serde::{Deserialize, Serialize};

use super::{
    DerivedValuation, EvidenceRecord, FundamentalData, MarketVolatilityData, NewsData,
    SentimentData, TechnicalData,
};

/// Equity-specific analyst outputs, evidence records, and derived artifacts.
///
/// Every field is optional because the equity pipeline populates them
/// incrementally and can legitimately skip subsets (e.g. a pack with only
/// `required_inputs = ["fundamentals"]` will leave sentiment / news /
/// technical as `None`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EquityState {
    /// Raw fundamentals analyst output.
    #[serde(default)]
    pub fundamental_metrics: Option<FundamentalData>,
    /// Raw technical analyst output.
    #[serde(default)]
    pub technical_indicators: Option<TechnicalData>,
    /// Raw sentiment analyst output.
    #[serde(default)]
    pub market_sentiment: Option<SentimentData>,
    /// Raw news analyst output (company news + macro events).
    #[serde(default)]
    pub macro_news: Option<NewsData>,

    /// Typed fundamentals evidence record (authoritative for evidence-aware
    /// readers).
    #[serde(default)]
    pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
    /// Typed technical evidence record.
    #[serde(default)]
    pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
    /// Typed sentiment evidence record.
    #[serde(default)]
    pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
    /// Typed news evidence record.
    #[serde(default)]
    pub evidence_news: Option<EvidenceRecord<NewsData>>,

    /// Market volatility snapshot derived from VIX.
    #[serde(default)]
    pub market_volatility: Option<MarketVolatilityData>,

    /// Derived valuation state (Chunk 1): deterministic valuation computed
    /// before trader inference.
    #[serde(default)]
    pub derived_valuation: Option<DerivedValuation>,
}
