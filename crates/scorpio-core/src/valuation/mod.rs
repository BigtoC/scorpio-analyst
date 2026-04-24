//! Pluggable valuation strategies keyed on [`AssetShape`].
//!
//! Phase 5 of the asset-class generalization refactor: valuation selection
//! moves from a hard-coded `ValuationAssessment` enum to a [`Valuator`]
//! trait + manifest-selected [`ValuatorId`], so packs can swap in
//! crypto-native valuation models (network-value, tokenomics-discount)
//! without forking `state::valuation_derive::derive_valuation`.
//!
//! # Scope in this slice
//!
//! - [`Valuator`] trait with an `assess` entry point that returns a
//!   [`ValuationReport`].
//! - [`ValuatorId`] enum keyed on `AssetShape` family so the manifest
//!   carries a stable selection key, not a trait object.
//! - [`ValuatorRegistry`] resolves `ValuatorId → Arc<dyn Valuator>`.
//! - [`equity::EquityDefaultValuator`] is a thin wrapper around the
//!   existing `derive_valuation` so the 16 existing tests keep passing
//!   and no behaviour changes for baseline equity runs.
//! - Crypto variants (`CryptoTokenomics`, `CryptoNetworkValue`) are
//!   registered but return `NotAssessed` until the crypto pack lands.
pub mod equity;
pub mod registry;

pub use equity::EquityDefaultValuator;
pub use registry::ValuatorRegistry;

use serde::{Deserialize, Serialize};

use crate::state::{AssetShape, DerivedValuation};

/// Stable manifest-facing identifier for a valuation strategy.
///
/// `#[non_exhaustive]` so adding crypto valuators is a non-breaking change
/// for external packs. Serde uses `rename_all = "snake_case"` so manifest
/// TOML can reference these as `equity_default`, `crypto_tokenomics`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValuatorId {
    /// Equity default — composes DCF + multiples + forward P/E + PEG.
    EquityDefault,
    /// Placeholder — crypto tokenomics-based valuation.
    CryptoTokenomics,
    /// Placeholder — network-value-based crypto valuation.
    CryptoNetworkValue,
}

/// The output of a valuation assessment.
///
/// Today this is a straight alias of [`DerivedValuation`] so the existing
/// `AnalystSyncTask` integration stays byte-identical. When Phase 6
/// reshapes `TradingState` the report type may grow asset-class-specific
/// variants; keeping a dedicated name here gives us a stable boundary for
/// that later change.
pub type ValuationReport = DerivedValuation;

/// Strategy that produces a [`ValuationReport`] for a given asset shape.
///
/// The inputs to `assess` are intentionally narrow in this slice — the
/// equity default consumes the same Yahoo Finance financial statement data
/// `derive_valuation` already accepts, packed in a typed [`ValuationInputs`]
/// carrier. Crypto valuators will grow their own input type when their
/// implementation lands; the trait is kept generic enough to let them
/// ignore fields they don't care about.
pub trait Valuator: Send + Sync {
    /// Canonical id for this strategy (used by registries and logs).
    fn id(&self) -> ValuatorId;

    /// Run the valuation.
    ///
    /// Implementations should never panic; missing inputs map to a
    /// `ValuationReport::NotAssessed { reason }` result rather than an
    /// `Err`, matching the existing graceful-degradation contract in
    /// `derive_valuation`.
    fn assess(&self, inputs: ValuationInputs<'_>, shape: &AssetShape) -> ValuationReport;
}

/// Typed carrier for the financial-statement inputs consumed by equity
/// valuators. Mirrors the arguments of `derive_valuation` exactly so the
/// shim can forward without reshaping.
pub struct ValuationInputs<'a> {
    pub profile: Option<yfinance_rs::profile::Profile>,
    pub cashflow: Option<&'a [yfinance_rs::fundamentals::CashflowRow]>,
    pub balance: Option<&'a [yfinance_rs::fundamentals::BalanceSheetRow]>,
    pub income: Option<&'a [yfinance_rs::fundamentals::IncomeStatementRow]>,
    pub shares: Option<&'a [yfinance_rs::fundamentals::ShareCount]>,
    pub earnings_trend: Option<&'a [yfinance_rs::analysis::EarningsTrendRow]>,
    pub current_price: Option<f64>,
}
