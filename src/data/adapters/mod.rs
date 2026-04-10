//! Enrichment adapter contracts and concrete provider implementations.
//!
//! This module declares [`ProviderCapabilities`], the three provider-trait
//! seams for transcripts, consensus estimates, and event news, and the
//! [`EnrichmentResult`] three-state type used at the adapter boundary.
//!
//! Concrete providers:
//! - [`events::FinnhubEventNewsProvider`] — event-news via Finnhub company news
//! - [`estimates::YFinanceEstimatesProvider`] — consensus estimates via yfinance-rs
//! - Transcripts: contract-only seam (deferred to a future plan)

pub mod estimates;
pub mod events;
pub mod transcripts;

use serde::{Deserialize, Serialize};

use crate::config::DataEnrichmentConfig;

/// Persisted status for an enrichment category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EnrichmentStatus {
    /// Enrichment category is disabled by configuration.
    Disabled,
    /// Enrichment category is not configured or intentionally skipped.
    NotConfigured,
    /// Provider returned no usable data for the requested scope.
    NotAvailable,
    /// Provider fetch failed or timed out.
    FetchFailed(String),
    /// Provider returned usable data.
    Available,
}

/// Three-state enrichment result that distinguishes "data available" from
/// "no data exists" from "fetch failed."
///
/// Consumers should treat [`EnrichmentResult::FetchFailed`] the same as
/// [`EnrichmentResult::NotAvailable`] for fail-open semantics, but can log
/// or surface the failure reason for observability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EnrichmentResult<T> {
    /// The provider returned usable data.
    Available(T),
    /// The provider confirmed no data exists for the requested scope.
    NotAvailable,
    /// The fetch attempt failed with the given reason.
    FetchFailed(String),
}

impl<T> EnrichmentResult<T> {
    /// Convert to `Option<T>`, collapsing both absence and failure to `None`.
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Available(v) => Some(v),
            Self::NotAvailable | Self::FetchFailed(_) => None,
        }
    }

    /// Returns `true` if the result contains available data.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    /// Convert this transient fetch result into a persisted status value.
    pub fn status(&self) -> EnrichmentStatus {
        match self {
            Self::Available(_) => EnrichmentStatus::Available,
            Self::NotAvailable => EnrichmentStatus::NotAvailable,
            Self::FetchFailed(reason) => EnrichmentStatus::FetchFailed(reason.clone()),
        }
    }
}

/// Runtime enrichment capabilities derived from [`DataEnrichmentConfig`].
///
/// Constructed once during preflight via [`ProviderCapabilities::from_config`]
/// and written to workflow context so all downstream tasks can read it without
/// having to re-inspect the config themselves.
///
/// Stage 1: all flags are boolean projections from the config.  No live API
/// call is needed — the spec is explicit that "capability discovery itself
/// cannot fail in the first slice because it is config-derived only."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Whether transcript evidence is enabled for this run.
    pub transcripts: bool,
    /// Whether consensus-estimates evidence is enabled for this run.
    pub consensus_estimates: bool,
    /// Whether event-news evidence is enabled for this run.
    pub event_news: bool,
}

impl ProviderCapabilities {
    /// Derive capabilities from the checked-in enrichment configuration.
    ///
    /// This constructor is infallible: reading from a fully-loaded
    /// `DataEnrichmentConfig` cannot fail.
    pub fn from_config(cfg: &DataEnrichmentConfig) -> Self {
        Self {
            transcripts: cfg.enable_transcripts,
            consensus_estimates: cfg.enable_consensus_estimates,
            event_news: cfg.enable_event_news,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DataEnrichmentConfig;

    // ── EnrichmentResult tests ───────────────────────────────────────────

    #[test]
    fn enrichment_result_available_roundtrips() {
        let result: EnrichmentResult<String> = EnrichmentResult::Available("hello".to_owned());
        let json = serde_json::to_string(&result).expect("serialize");
        let recovered: EnrichmentResult<String> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, recovered);
    }

    #[test]
    fn enrichment_result_not_available_roundtrips() {
        let result: EnrichmentResult<String> = EnrichmentResult::NotAvailable;
        let json = serde_json::to_string(&result).expect("serialize");
        let recovered: EnrichmentResult<String> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, recovered);
    }

    #[test]
    fn enrichment_result_fetch_failed_roundtrips() {
        let result: EnrichmentResult<String> = EnrichmentResult::FetchFailed("timeout".to_owned());
        let json = serde_json::to_string(&result).expect("serialize");
        let recovered: EnrichmentResult<String> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, recovered);
    }

    #[test]
    fn enrichment_result_to_option_available() {
        let result = EnrichmentResult::Available(42);
        assert_eq!(result.into_option(), Some(42));
    }

    #[test]
    fn enrichment_result_to_option_not_available() {
        let result: EnrichmentResult<i32> = EnrichmentResult::NotAvailable;
        assert_eq!(result.into_option(), None);
    }

    #[test]
    fn enrichment_result_to_option_fetch_failed() {
        let result: EnrichmentResult<i32> = EnrichmentResult::FetchFailed("err".to_owned());
        assert_eq!(result.into_option(), None);
    }

    // ── ProviderCapabilities tests ───────────────────────────────────────

    #[test]
    fn from_config_all_disabled_produces_all_false() {
        let cfg = DataEnrichmentConfig::default();
        let caps = ProviderCapabilities::from_config(&cfg);
        assert!(!caps.transcripts);
        assert!(!caps.consensus_estimates);
        assert!(!caps.event_news);
    }

    #[test]
    fn from_config_transcripts_only() {
        let cfg = DataEnrichmentConfig {
            enable_transcripts: true,
            ..DataEnrichmentConfig::default()
        };
        let caps = ProviderCapabilities::from_config(&cfg);
        assert!(caps.transcripts);
        assert!(!caps.consensus_estimates);
        assert!(!caps.event_news);
    }

    #[test]
    fn from_config_all_enabled() {
        let cfg = DataEnrichmentConfig {
            enable_transcripts: true,
            enable_consensus_estimates: true,
            enable_event_news: true,
            max_evidence_age_hours: 12,
            ..DataEnrichmentConfig::default()
        };
        let caps = ProviderCapabilities::from_config(&cfg);
        assert!(caps.transcripts);
        assert!(caps.consensus_estimates);
        assert!(caps.event_news);
    }

    #[test]
    fn provider_capabilities_serializes_and_deserializes() {
        let caps = ProviderCapabilities {
            transcripts: true,
            consensus_estimates: false,
            event_news: true,
        };
        let json = serde_json::to_string(&caps).expect("serialization");
        let recovered: ProviderCapabilities = serde_json::from_str(&json).expect("deserialization");
        assert_eq!(caps, recovered);
    }
}
