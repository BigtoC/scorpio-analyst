use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::{
    DataCoverageReport, EvidenceRecord, ExecutionStatus, FundamentalData, MarketVolatilityData,
    NewsData, ProvenanceSummary, RiskReport, SentimentData, TechnicalData, TokenUsageTracker,
    TradeProposal,
};

/// A single message entry in a debate or risk discussion history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebateMessage {
    /// The speaker role, e.g. `"bullish_researcher"`, `"bearish_researcher"`, or `"moderator"`.
    pub role: String,
    /// The free-text content of the message produced by the LLM agent.
    pub content: String,
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
