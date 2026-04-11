use serde::{Deserialize, Serialize};

/// Strategy lens for prompt and report framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyFocus {
    /// Balanced institutional — weights all data sources equally.
    Balanced,
    /// Deep value — emphasizes DCF, earnings quality, margin of safety.
    DeepValue,
    /// Momentum — emphasizes price action, flow, and trend signals.
    Momentum,
}

/// Asset-shape valuation assessment policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationAssessment {
    /// Full deterministic valuation (DCF, multiples) for corporate equities.
    Full,
    /// Valuation not assessed — explicit fallback for ETFs, indices, etc.
    NotAssessed,
}
