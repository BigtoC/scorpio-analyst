//! Declarative analysis-pack layer.
//!
//! Sits above config, evidence policy, prompt policy, and report policy.
//! Packs shape analysis behavior without changing the five-phase graph
//! or provider-factory routing.
//!
//! # Module layout
//!
//! - [`manifest`] — schema + validation for [`AnalysisPackManifest`]
//!   ([`PackId`], [`EnrichmentIntent`], [`StrategyFocus`],
//!   [`ValuationAssessment`]).
//! - [`equity`] — the baseline equity pack definition.
//! - [`crypto`] — crypto-pack stubs (non-selectable in this slice).
//! - [`registry`] — [`resolve_pack`] — single entry point for
//!   `PackId` → manifest resolution.
//! - [`selection`] — runtime policy hydration from the manifest
//!   ([`RuntimePolicy`], [`resolve_runtime_policy`]).

mod crypto;
mod equity;
mod manifest;
mod registry;
mod selection;

pub use manifest::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};
pub use registry::resolve_pack;
pub use selection::{RuntimePolicy, resolve_runtime_policy};
pub(crate) use selection::resolve_runtime_policy_for_manifest;
