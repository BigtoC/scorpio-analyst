//! Declarative analysis-pack layer.
//!
//! Sits above config, evidence policy, prompt policy, and report policy.
//! Packs shape analysis behavior without changing the five-phase graph
//! or provider-factory routing.
//!
//! First-slice: built-in packs only, selected by config/env.

pub mod builtin;
pub mod manifest;
pub mod selection;

pub use builtin::resolve_pack;
pub use manifest::{
    AnalysisPackManifest, EnrichmentIntent, PackId, StrategyFocus, ValuationAssessment,
};
pub use selection::{RuntimePolicy, resolve_runtime_policy};
