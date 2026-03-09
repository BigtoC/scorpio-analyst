use serde::{Deserialize, Serialize};

/// Revenue, earnings, valuation, and insider activity for the target asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FundamentalData {
    pub revenue_growth_pct: Option<f64>,
    pub pe_ratio: Option<f64>,
    pub eps: Option<f64>,
    pub current_ratio: Option<f64>,
    pub debt_to_equity: Option<f64>,
    pub gross_margin: Option<f64>,
    pub net_income: Option<f64>,
    pub insider_transactions: Vec<InsiderTransaction>,
    pub summary: String,
}

/// A single insider buy/sell record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsiderTransaction {
    pub name: String,
    pub share_change: f64,
    pub transaction_date: String,
    pub transaction_type: String,
}
