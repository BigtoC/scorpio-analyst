use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ExecutionStatus, FundamentalData, NewsData, RiskReport, SentimentData, TechnicalData,
    TokenUsageTracker, TradeProposal,
};

/// A single message entry in a debate or risk discussion history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebateMessage {
    pub role: String,
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

    // Phase 1: Aggregated analyst data
    pub fundamental_metrics: Option<FundamentalData>,
    pub technical_indicators: Option<TechnicalData>,
    pub market_sentiment: Option<SentimentData>,
    pub macro_news: Option<NewsData>,

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

impl TradingState {
    /// Create a new empty state for a trading cycle.
    pub fn new(asset_symbol: impl Into<String>, target_date: impl Into<String>) -> Self {
        Self {
            execution_id: Uuid::new_v4(),
            asset_symbol: asset_symbol.into(),
            target_date: target_date.into(),
            fundamental_metrics: None,
            technical_indicators: None,
            market_sentiment: None,
            macro_news: None,
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
}
