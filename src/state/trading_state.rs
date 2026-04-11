use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::data::adapters::{
    EnrichmentStatus, estimates::ConsensusEvidence, events::EventNewsEvidence,
};

use super::{
    DataCoverageReport, DerivedValuation, EvidenceRecord, ExecutionStatus, FundamentalData,
    MarketVolatilityData, NewsData, ProvenanceSummary, RiskReport, SentimentData, TechnicalData,
    ThesisMemory, TokenUsageTracker, TradeProposal,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradingState {
    pub execution_id: Uuid,
    pub asset_symbol: String,
    pub target_date: String,

    // Market price and volatility context at the time of analysis
    pub current_price: Option<f64>,
    pub market_volatility: Option<MarketVolatilityData>,

    // Phase 1: Aggregated analyst data (legacy mirrors — stay for Stage 1 dual-write)
    pub fundamental_metrics: Option<FundamentalData>,
    pub technical_indicators: Option<TechnicalData>,
    pub market_sentiment: Option<SentimentData>,
    pub macro_news: Option<NewsData>,

    // Phase 1: Typed evidence records (authoritative for new evidence-aware readers)
    pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
    pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
    pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
    pub evidence_news: Option<EvidenceRecord<NewsData>>,

    // Enrichment data (hydrated in run_analysis_cycle when enabled)
    #[serde(default)]
    pub enrichment_event_news: EnrichmentState<Vec<EventNewsEvidence>>,
    #[serde(default)]
    pub enrichment_consensus: EnrichmentState<ConsensusEvidence>,

    // Phase 1: Run-level coverage and provenance reporting
    pub data_coverage: Option<DataCoverageReport>,
    pub provenance_summary: Option<ProvenanceSummary>,

    // Phase 2: Dialectical debate
    pub debate_history: Vec<DebateMessage>,
    pub consensus_summary: Option<String>,

    // Phase 3 & 4: Synthesis and risk
    pub trader_proposal: Option<TradeProposal>,
    pub risk_discussion_history: Vec<DebateMessage>,
    pub aggressive_risk_report: Option<RiskReport>,
    pub neutral_risk_report: Option<RiskReport>,
    pub conservative_risk_report: Option<RiskReport>,

    // Phase 5: Final execution
    pub final_execution_status: Option<ExecutionStatus>,

    // Thesis memory: prior-run context (loaded in preflight) and
    // current-run capture (set by FundManagerTask before final snapshot save).
    // Both are `None` at cycle start; `prior_thesis` is populated by preflight
    // if a compatible prior run exists; `current_thesis` is always reset by
    // `reset_cycle_outputs` so stale data never leaks across reused runs.
    #[serde(default)]
    pub prior_thesis: Option<ThesisMemory>,
    #[serde(default)]
    pub current_thesis: Option<ThesisMemory>,

    // Derived valuation state (Chunk 1): deterministic valuation computed before
    // trader inference.  `None` at cycle start and on pre-feature snapshots.
    // Must be reset by `reset_cycle_outputs` to prevent stale values leaking
    // across reused pipeline runs.
    #[serde(default)]
    pub derived_valuation: Option<DerivedValuation>,

    // Analysis pack metadata: lightweight pack name persisted for forward
    // compatibility. Full version tracking deferred to a follow-on slice.
    // `None` for old snapshots or runs before pack extraction.
    #[serde(default)]
    pub analysis_pack_name: Option<String>,

    // Token accounting
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
    pub fn new(asset_symbol: impl Into<String>, target_date: impl Into<String>) -> Self {
        Self {
            execution_id: Uuid::new_v4(),
            asset_symbol: asset_symbol.into(),
            target_date: target_date.into(),
            current_price: None,
            market_volatility: None,
            fundamental_metrics: None,
            technical_indicators: None,
            market_sentiment: None,
            macro_news: None,
            evidence_fundamental: None,
            evidence_technical: None,
            evidence_sentiment: None,
            evidence_news: None,
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
            derived_valuation: None,
            analysis_pack_name: None,
            token_usage: TokenUsageTracker::default(),
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
            self.fundamental_metrics.is_none()
                && self.technical_indicators.is_none()
                && self.market_sentiment.is_none()
                && self.macro_news.is_none(),
            "analyst_handles() called on a TradingState that already has analyst data; \
             did you forget to call TradingState::new() for this analysis cycle?"
        );
        // The four clones below are near-zero cost: all fields are None at this
        // point (enforced by the debug_assert above), so each clone is just
        // Option::None — a single-byte copy with no heap allocation.
        AnalystStateHandles {
            fundamental_metrics: Arc::new(RwLock::new(self.fundamental_metrics.clone())),
            technical_indicators: Arc::new(RwLock::new(self.technical_indicators.clone())),
            market_sentiment: Arc::new(RwLock::new(self.market_sentiment.clone())),
            macro_news: Arc::new(RwLock::new(self.macro_news.clone())),
        }
    }

    /// Merge concurrent analyst results back into the main state after fan-out completes.
    pub async fn apply_analyst_handles(&mut self, handles: &AnalystStateHandles) {
        // All four tasks have already finished (their JoinHandles were awaited before
        // this call), so these `.read()` locks are uncontested: no task holds a write
        // lock at this point.  They still use `.await` to satisfy the async RwLock API.
        self.fundamental_metrics = handles.fundamental_metrics.read().await.clone();
        self.technical_indicators = handles.technical_indicators.read().await.clone();
        self.market_sentiment = handles.market_sentiment.read().await.clone();
        self.macro_news = handles.macro_news.read().await.clone();
    }
}
