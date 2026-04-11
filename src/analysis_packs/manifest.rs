//! Analysis-pack schema and validation.
//!
//! Defines the declarative analysis-pack vocabulary: coverage, enrichment
//! intent, strategy focus, valuation policy, and pack metadata.
//! Packs are policy objects — they shape analysis behavior without owning
//! execution or graph topology.

mod pack_id;
mod schema;
mod strategy;

pub use pack_id::PackId;
pub use schema::{AnalysisPackManifest, EnrichmentIntent};
pub use strategy::{StrategyFocus, ValuationAssessment};

#[cfg(test)]
mod tests;
