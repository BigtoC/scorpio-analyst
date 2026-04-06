//! Stage 1 enrichment adapter contracts.
//!
//! This module declares [`ProviderCapabilities`] and the three provider-trait
//! seams for transcripts, consensus estimates, and event news.  In Stage 1 the
//! capabilities are config-derived only (no live API discovery); concrete
//! provider implementations are deferred to Milestone 7.

pub mod estimates;
pub mod events;
pub mod transcripts;

use serde::{Deserialize, Serialize};

use crate::config::DataEnrichmentConfig;

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
