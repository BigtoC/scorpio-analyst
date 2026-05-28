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
pub mod etf;
pub mod registry;

pub use equity::EquityDefaultValuator;
pub use etf::EtfPremiumDiscountValuator;
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
    /// ETF premium/discount + composition + tracking valuator.
    EtfPremiumDiscount,
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

    // ETF inputs (None when active pack != EtfBaseline)
    pub etf_quote: Option<&'a crate::data::yfinance::etf::EtfQuote>,
    pub etf_fund_info: Option<&'a crate::data::yfinance::etf::FundInfo>,
    pub etf_holdings: Option<&'a crate::data::sec_edgar_nport::NPortHoldings>,
    pub etf_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,
    pub etf_benchmark_ohlcv: Option<&'a [crate::data::yfinance::Candle]>,

    /// Phase 2 — Live ETF options snapshot threaded through from the persisted
    /// `TechnicalOptionsContext` before valuation runs. `None` when no snapshot
    /// is available or active pack is not `EtfBaseline`.
    pub etf_options: Option<&'a crate::data::traits::options::OptionsSnapshot>,

    /// Phase 2 (Stage 2) — FRED `DGS3MO` snapshot threaded from preflight when
    /// the active pack is `EtfBaseline`, or yfinance `^IRX` when FRED is
    /// unavailable. `None` when pack != EtfBaseline OR when both live rate
    /// sources failed. The ETF valuator must degrade dealer-positioning to
    /// `None`; no hardcoded risk-free-rate fallback is allowed.
    pub etf_risk_free_rate: Option<f64>,

    /// Phase 2 — trailing distribution yield in decimal units (e.g. 0.015 for
    /// 1.5%), used as continuous dividend yield `q` in ETF options Greeks.
    pub etf_distribution_yield_ttm: Option<f64>,

    /// Phase 2 — Reference date for time-to-expiration math, sourced from
    /// `state.target_date`. Defaulted to `chrono::Utc::now().date_naive()` by
    /// the equity path which does not read it.
    pub as_of: chrono::NaiveDate,
}
