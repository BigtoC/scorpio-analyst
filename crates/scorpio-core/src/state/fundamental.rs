use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Revenue, earnings, valuation, and insider activity for the target asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// Whether an insider bought (`P`) or sold (`S`) shares.
///
/// `S` and `P` are the transaction codes used in Finnhub / SEC Form 4 filings.
/// Unknown codes (option exercises, gifts, etc.) are captured by `Other` without
/// discarding the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TransactionType {
    /// Purchase / buy.
    S,
    /// Sale / sell.
    P,
    /// Any other SEC Form 4 transaction code not explicitly modelled.
    #[serde(other)]
    Other,
}

/// A single insider buy/sell record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InsiderTransaction {
    pub name: String,
    pub share_change: f64,
    pub transaction_date: String,
    pub transaction_type: TransactionType,
}
