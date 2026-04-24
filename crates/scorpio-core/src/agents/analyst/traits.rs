//! Analyst identity, data-need taxonomy, and the thin [`Analyst`] trait that
//! lets the pipeline compose a fan-out from pack metadata at runtime.
//!
//! The trait stays intentionally narrow in this Phase 2 slice — analyst
//! *execution* still happens through the existing per-analyst `Task`
//! implementations (see `workflow/tasks/analyst.rs`). The trait is a metadata
//! handle used by the registry so `TradingPipeline::build_graph` can look up
//! which analyst to spawn for each `required_inputs` entry on the active
//! pack.
//!
//! A richer `AnalystOutput` sum type + dispatch-via-trait lands in Phase 6
//! once `TradingState` is reshaped; today's concrete analyst outputs still
//! flow through bespoke typed fields on state.
use std::fmt;

use serde::{Deserialize, Serialize};

/// Canonical identifier for every analyst the system knows about.
///
/// Marked `#[non_exhaustive]` so future analyst additions remain a
/// non-breaking change for external packs and other downstream crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnalystId {
    // ── Equity analysts (live today) ──────────────────────────────────────
    /// Fundamentals — earnings, ratios, insider activity (Finnhub).
    Fundamental,
    /// Sentiment — news-derived sentiment scoring (Finnhub).
    Sentiment,
    /// News — articles and macro events (Finnhub + FRED).
    News,
    /// Technical — OHLCV → indicator summary (Yahoo Finance).
    Technical,
    // ── Crypto analysts (placeholders; implemented in crypto pack slice) ─
    /// Token supply, unlock schedules, treasury (crypto pack placeholder).
    Tokenomics,
    /// On-chain flows, holder concentration (crypto pack placeholder).
    OnChain,
    /// Social / community signals (crypto pack placeholder).
    Social,
    /// Derivatives — perp funding, OI, basis (crypto pack placeholder).
    Derivatives,
}

impl AnalystId {
    /// Map a pack `required_inputs` entry to the analyst that satisfies it.
    ///
    /// Returns `None` for unknown input names so callers can degrade
    /// gracefully (matching the existing behaviour in `workflow/tasks/analyst.rs`
    /// where unknown entries fall through `input_missing`).
    #[must_use]
    pub fn from_required_input(input: &str) -> Option<Self> {
        match input {
            "fundamentals" => Some(Self::Fundamental),
            "sentiment" => Some(Self::Sentiment),
            "news" => Some(Self::News),
            "technical" => Some(Self::Technical),
            "tokenomics" => Some(Self::Tokenomics),
            "onchain" => Some(Self::OnChain),
            "social" => Some(Self::Social),
            "derivatives" => Some(Self::Derivatives),
            _ => None,
        }
    }

    /// Human-readable display name — matches the strings previously used in
    /// analyst error messages and token-usage entries so log/telemetry output
    /// is byte-identical for the equity baseline.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Fundamental => "Fundamental Analyst",
            Self::Sentiment => "Sentiment Analyst",
            Self::News => "News Analyst",
            Self::Technical => "Technical Analyst",
            Self::Tokenomics => "Tokenomics Analyst",
            Self::OnChain => "On-Chain Analyst",
            Self::Social => "Social Analyst",
            Self::Derivatives => "Derivatives Analyst",
        }
    }

    /// Short context-key slug — matches the strings in
    /// `workflow/tasks/common::ANALYST_*` so the existing snapshot keys keep
    /// the same shape for baseline runs.
    #[must_use]
    pub fn context_key(self) -> &'static str {
        match self {
            Self::Fundamental => "fundamental",
            Self::Sentiment => "sentiment",
            Self::News => "news",
            Self::Technical => "technical",
            Self::Tokenomics => "tokenomics",
            Self::OnChain => "onchain",
            Self::Social => "social",
            Self::Derivatives => "derivatives",
        }
    }
}

impl fmt::Display for AnalystId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Upstream data category an analyst consumes.
///
/// Coarse-grained per Decision D2 — matches today's `required_inputs`
/// vocabulary so pack manifests written against the string form map 1:1.
/// Fine-grained sub-needs (e.g. forward-EPS only) are a v2 concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DataNeed {
    /// Corporate fundamentals — ratios, earnings, insider trades.
    Fundamentals,
    /// Company news with sentiment scoring.
    Sentiment,
    /// General news and macro events.
    News,
    /// Historical price / OHLCV data.
    PriceHistory,
    /// Macroeconomic indicators (rates, inflation, etc.).
    Macro,
    /// Token supply, unlocks, treasury data.
    Tokenomics,
    /// On-chain activity and holder concentration.
    OnChain,
    /// Derivatives markets (funding rates, open interest).
    Derivatives,
    /// Social / community sentiment signals.
    Social,
}

/// Marker / metadata trait implemented by every analyst so a registry can
/// look them up by [`AnalystId`] and ask what data they need.
///
/// Execution itself still goes through per-analyst `Task` implementations in
/// `workflow/tasks/analyst.rs`. When `AnalystOutput` lands in Phase 6 this
/// trait will gain a `run()`-style method and the per-analyst `Task` types
/// collapse into a registry-driven adapter.
pub trait Analyst: Send + Sync {
    /// Canonical identifier used by pack manifests and registry lookups.
    fn id(&self) -> AnalystId;

    /// Data needs this analyst pulls from upstream providers.
    fn required_data(&self) -> Vec<DataNeed>;
}
