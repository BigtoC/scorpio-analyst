use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::analysis_packs::RuntimePolicy;
use crate::data::adapters::{
    EnrichmentStatus, estimates::ConsensusEvidence, events::EventNewsEvidence,
};
use crate::domain::Symbol;

use super::{
    CryptoState, DataCoverageReport, DerivedValuation, EquityState, EvidenceRecord,
    ExecutionStatus, FundamentalData, MarketVolatilityData, NewsData, ProvenanceSummary,
    RiskReport, SentimentData, TechnicalData, ThesisMemory, TokenUsageTracker, TradeProposal,
};

/// A single message entry in a debate or risk discussion history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebateMessage {
    /// The speaker role, e.g. `"bullish_researcher"`, `"bearish_researcher"`, or `"moderator"`.
    pub role: String,
    /// The free-text content of the message produced by the LLM agent.
    pub content: String,
}

/// Persisted enrichment state for a single category.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnrichmentState<T> {
    pub status: EnrichmentStatus,
    pub payload: Option<T>,
}

impl<'de, T> Deserialize<'de> for EnrichmentState<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct EnrichmentStateFields<T> {
            status: EnrichmentStatus,
            payload: Option<T>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum EnrichmentStateRepr<T> {
            State(EnrichmentStateFields<T>),
            LegacyPayload(T),
            Null(()),
        }

        match EnrichmentStateRepr::deserialize(deserializer)? {
            EnrichmentStateRepr::State(fields) => Ok(Self {
                status: fields.status,
                payload: fields.payload,
            }),
            EnrichmentStateRepr::LegacyPayload(payload) => Ok(Self {
                status: EnrichmentStatus::Available,
                payload: Some(payload),
            }),
            EnrichmentStateRepr::Null(()) => Ok(Self::default()),
        }
    }
}

impl<T> Default for EnrichmentState<T> {
    fn default() -> Self {
        Self {
            status: EnrichmentStatus::NotConfigured,
            payload: None,
        }
    }
}

/// The unified shared state that flows through every phase of the trading pipeline.
///
/// Populated incrementally: analyst data is written during fan-out, debate history
/// grows through cyclic rounds, and the final execution status is set by the Fund Manager.
///
/// # Phase 6 shape
///
/// Equity-scoped analyst outputs, evidence records, volatility, and derived
/// valuation live under [`Self::equity`]. Crypto-scoped state lives under
/// [`Self::crypto`] and is always `None` in this slice (the crypto pack
/// slice wires it up). Shared fields — identity, debate, trader proposal,
/// risk reports, thesis memory, token usage — remain at the root.
///
/// Reader sites should use the accessor methods ([`Self::fundamental_metrics`],
/// [`Self::set_fundamental_metrics`], …) rather than pattern-matching on
/// `equity` directly, so a later cleanup that retires the string-form
/// `asset_symbol` or reshapes storage further stays source-compatible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradingState {
    pub execution_id: Uuid,
    /// Raw string form of the instrument symbol. Transitional mirror of
    /// [`Self::symbol`]; see `set_symbol` for the single write path that
    /// keeps the two in sync.
    pub asset_symbol: String,
    /// Typed, class-aware instrument identity.
    #[serde(default)]
    pub symbol: Option<Symbol>,
    pub target_date: String,

    /// Market price at the time of analysis — shared across asset classes.
    pub current_price: Option<f64>,

    /// Equity-scoped analyst outputs, evidence, volatility, valuation.
    /// `None` when the active pack is not equity (crypto runs, future
    /// classes) or on pre-Phase-6 snapshots. Access via accessor methods.
    #[serde(default)]
    pub equity: Option<EquityState>,
    /// Crypto-scoped sub-state — placeholder in this slice, always `None`.
    #[serde(default)]
    pub crypto: Option<CryptoState>,

    /// Enrichment data (hydrated in run_analysis_cycle when enabled). These
    /// stay at root because enrichment policy is pack-driven but the payload
    /// shape (Finnhub-style events, consensus estimates) is equity-specific
    /// today — the crypto pack will introduce crypto-native enrichment
    /// types rather than reshape these slots.
    #[serde(default)]
    pub enrichment_event_news: EnrichmentState<Vec<EventNewsEvidence>>,
    #[serde(default)]
    pub enrichment_consensus: EnrichmentState<ConsensusEvidence>,

    /// Run-level coverage and provenance reporting — shared reporting
    /// concerns independent of asset class.
    pub data_coverage: Option<DataCoverageReport>,
    pub provenance_summary: Option<ProvenanceSummary>,

    /// Phase 2: Dialectical debate.
    pub debate_history: Vec<DebateMessage>,
    pub consensus_summary: Option<String>,

    /// Phase 3 & 4: Synthesis and risk.
    pub trader_proposal: Option<TradeProposal>,
    pub risk_discussion_history: Vec<DebateMessage>,
    pub aggressive_risk_report: Option<RiskReport>,
    pub neutral_risk_report: Option<RiskReport>,
    pub conservative_risk_report: Option<RiskReport>,

    /// Phase 5: Final execution.
    pub final_execution_status: Option<ExecutionStatus>,

    /// Thesis memory: prior-run context + current-run capture.
    #[serde(default)]
    pub prior_thesis: Option<ThesisMemory>,
    #[serde(default)]
    pub current_thesis: Option<ThesisMemory>,

    /// Lightweight pack name persisted for forward compatibility.
    #[serde(default)]
    pub analysis_pack_name: Option<String>,

    /// Resolved pack-derived runtime policy.
    #[serde(default)]
    pub analysis_runtime_policy: Option<RuntimePolicy>,

    /// Token accounting.
    pub token_usage: TokenUsageTracker,
}

/// Concurrent write handles for the analyst-owned Phase 1 fields.
#[derive(Debug, Clone)]
pub struct AnalystStateHandles {
    pub fundamental_metrics: Arc<RwLock<Option<FundamentalData>>>,
    pub technical_indicators: Arc<RwLock<Option<TechnicalData>>>,
    pub market_sentiment: Arc<RwLock<Option<SentimentData>>>,
    pub macro_news: Arc<RwLock<Option<NewsData>>>,
}

impl TradingState {
    /// Create a new empty state for a trading cycle.
    ///
    /// The raw symbol string is parsed into a typed [`Symbol`] via
    /// [`Symbol::parse`]; on success the typed form becomes the source of
    /// truth and the stored `asset_symbol` is its canonical string rendering.
    /// On parse failure the raw input is preserved and `symbol` is `None` so
    /// fixture-driven callers that pass deliberately unusual strings still
    /// succeed.
    pub fn new(asset_symbol: impl Into<String>, target_date: impl Into<String>) -> Self {
        let raw = asset_symbol.into();
        let symbol = Symbol::parse(&raw).ok();
        let asset_symbol = match &symbol {
            Some(s) => s.to_string(),
            None => raw,
        };
        Self {
            execution_id: Uuid::new_v4(),
            asset_symbol,
            symbol,
            target_date: target_date.into(),
            current_price: None,
            equity: None,
            crypto: None,
            enrichment_event_news: EnrichmentState::default(),
            enrichment_consensus: EnrichmentState::default(),
            data_coverage: None,
            provenance_summary: None,
            debate_history: Vec::new(),
            consensus_summary: None,
            trader_proposal: None,
            risk_discussion_history: Vec::new(),
            aggressive_risk_report: None,
            neutral_risk_report: None,
            conservative_risk_report: None,
            final_execution_status: None,
            prior_thesis: None,
            current_thesis: None,
            analysis_pack_name: None,
            analysis_runtime_policy: None,
            token_usage: TokenUsageTracker::default(),
        }
    }

    /// Set the instrument identity from a typed [`Symbol`], keeping the raw
    /// `asset_symbol` mirror in sync.
    pub fn set_symbol(&mut self, symbol: Symbol) {
        self.asset_symbol = symbol.to_string();
        self.symbol = Some(symbol);
    }

    // ── Equity sub-state accessors ───────────────────────────────────────
    //
    // Accessor methods form the call-site API so sites stay source-compatible
    // through shape changes. `*_mut` variants lazily create the equity
    // sub-state so callers never need to pattern-match on `equity.is_none()`.

    /// Borrow the equity sub-state, if any.
    #[must_use]
    pub fn equity(&self) -> Option<&EquityState> {
        self.equity.as_ref()
    }

    /// Mutable borrow of the equity sub-state, creating it on demand.
    pub fn equity_mut(&mut self) -> &mut EquityState {
        self.equity.get_or_insert_with(EquityState::default)
    }

    /// Clear the equity sub-state and everything it carries.
    pub fn clear_equity(&mut self) {
        self.equity = None;
    }

    // ── Per-field equity accessors ────────────────────────────────────────

    #[must_use]
    pub fn fundamental_metrics(&self) -> Option<&FundamentalData> {
        self.equity.as_ref()?.fundamental_metrics.as_ref()
    }
    pub fn set_fundamental_metrics(&mut self, v: FundamentalData) {
        self.equity_mut().fundamental_metrics = Some(v);
    }
    pub fn clear_fundamental_metrics(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.fundamental_metrics = None;
        }
    }

    #[must_use]
    pub fn technical_indicators(&self) -> Option<&TechnicalData> {
        self.equity.as_ref()?.technical_indicators.as_ref()
    }
    pub fn set_technical_indicators(&mut self, v: TechnicalData) {
        self.equity_mut().technical_indicators = Some(v);
    }
    pub fn clear_technical_indicators(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.technical_indicators = None;
        }
    }

    #[must_use]
    pub fn market_sentiment(&self) -> Option<&SentimentData> {
        self.equity.as_ref()?.market_sentiment.as_ref()
    }
    pub fn set_market_sentiment(&mut self, v: SentimentData) {
        self.equity_mut().market_sentiment = Some(v);
    }
    pub fn clear_market_sentiment(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.market_sentiment = None;
        }
    }

    #[must_use]
    pub fn macro_news(&self) -> Option<&NewsData> {
        self.equity.as_ref()?.macro_news.as_ref()
    }
    pub fn set_macro_news(&mut self, v: NewsData) {
        self.equity_mut().macro_news = Some(v);
    }
    pub fn clear_macro_news(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.macro_news = None;
        }
    }

    #[must_use]
    pub fn evidence_fundamental(&self) -> Option<&EvidenceRecord<FundamentalData>> {
        self.equity.as_ref()?.evidence_fundamental.as_ref()
    }
    pub fn set_evidence_fundamental(&mut self, v: EvidenceRecord<FundamentalData>) {
        self.equity_mut().evidence_fundamental = Some(v);
    }

    #[must_use]
    pub fn evidence_technical(&self) -> Option<&EvidenceRecord<TechnicalData>> {
        self.equity.as_ref()?.evidence_technical.as_ref()
    }
    pub fn set_evidence_technical(&mut self, v: EvidenceRecord<TechnicalData>) {
        self.equity_mut().evidence_technical = Some(v);
    }

    #[must_use]
    pub fn evidence_sentiment(&self) -> Option<&EvidenceRecord<SentimentData>> {
        self.equity.as_ref()?.evidence_sentiment.as_ref()
    }
    pub fn set_evidence_sentiment(&mut self, v: EvidenceRecord<SentimentData>) {
        self.equity_mut().evidence_sentiment = Some(v);
    }

    #[must_use]
    pub fn evidence_news(&self) -> Option<&EvidenceRecord<NewsData>> {
        self.equity.as_ref()?.evidence_news.as_ref()
    }
    pub fn set_evidence_news(&mut self, v: EvidenceRecord<NewsData>) {
        self.equity_mut().evidence_news = Some(v);
    }

    #[must_use]
    pub fn market_volatility(&self) -> Option<&MarketVolatilityData> {
        self.equity.as_ref()?.market_volatility.as_ref()
    }
    pub fn set_market_volatility(&mut self, v: MarketVolatilityData) {
        self.equity_mut().market_volatility = Some(v);
    }
    pub fn clear_market_volatility(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.market_volatility = None;
        }
    }

    #[must_use]
    pub fn derived_valuation(&self) -> Option<&DerivedValuation> {
        self.equity.as_ref()?.derived_valuation.as_ref()
    }
    pub fn set_derived_valuation(&mut self, v: DerivedValuation) {
        self.equity_mut().derived_valuation = Some(v);
    }
    pub fn clear_derived_valuation(&mut self) {
        if let Some(e) = self.equity.as_mut() {
            e.derived_valuation = None;
        }
    }

    /// Create per-field async locks for concurrent analyst fan-out writes.
    ///
    /// **Invariant**: this method is intended for use at the start of Phase 1 when all
    /// analyst fields are `None`. The handles are seeded from the current field values,
    /// so calling this mid-pipeline (e.g. during backtesting multi-cycle reuse) would
    /// carry stale data into the new analysis cycle.
    #[must_use]
    pub fn analyst_handles(&self) -> AnalystStateHandles {
        debug_assert!(
            self.fundamental_metrics().is_none()
                && self.technical_indicators().is_none()
                && self.market_sentiment().is_none()
                && self.macro_news().is_none(),
            "analyst_handles() called on a TradingState that already has analyst data; \
             did you forget to call TradingState::new() for this analysis cycle?"
        );
        AnalystStateHandles {
            fundamental_metrics: Arc::new(RwLock::new(None)),
            technical_indicators: Arc::new(RwLock::new(None)),
            market_sentiment: Arc::new(RwLock::new(None)),
            macro_news: Arc::new(RwLock::new(None)),
        }
    }

    /// Merge concurrent analyst results back into the main state after fan-out completes.
    pub async fn apply_analyst_handles(&mut self, handles: &AnalystStateHandles) {
        if let Some(f) = handles.fundamental_metrics.read().await.clone() {
            self.set_fundamental_metrics(f);
        }
        if let Some(t) = handles.technical_indicators.read().await.clone() {
            self.set_technical_indicators(t);
        }
        if let Some(s) = handles.market_sentiment.read().await.clone() {
            self.set_market_sentiment(s);
        }
        if let Some(n) = handles.macro_news.read().await.clone() {
            self.set_macro_news(n);
        }
    }
}
