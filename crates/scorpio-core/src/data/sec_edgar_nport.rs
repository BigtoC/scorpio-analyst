//! Lightweight types for SEC N-PORT-P holdings (placeholder).
//!
//! The XBRL parser lands in Task 9; this file declares the shape so
//! downstream code can compile.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortHoldingRow {
    pub cusip: Option<String>,
    pub ticker: Option<String>,
    pub name: String,
    pub weight_pct: f64,
    pub value_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortSectorRow {
    pub sector: String,
    pub weight_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NPortHoldings {
    pub filing_date: NaiveDate,
    #[serde(default)]
    pub report_date: Option<NaiveDate>,
    pub holdings: Vec<NPortHoldingRow>,
    pub sector_breakdown: Vec<NPortSectorRow>,
    pub stated_benchmark: Option<String>,
}
