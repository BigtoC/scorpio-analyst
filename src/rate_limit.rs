use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{Quota, RateLimiter};

use crate::config::{ProvidersConfig, RateLimitConfig};
use crate::providers::ProviderId;

/// A shared, async-aware rate limiter backed by `governor`.
///
/// Wrap in `Arc` and inject into data clients and agent tasks to
/// enforce per-provider request quotas across concurrent operations.
#[derive(Debug, Clone)]
pub struct SharedRateLimiter {
    inner: Option<
        Arc<
            RateLimiter<
                governor::state::NotKeyed,
                governor::state::InMemoryState,
                governor::clock::DefaultClock,
            >,
        >,
    >,
    label: String,
}

impl SharedRateLimiter {
    /// Create a new rate limiter allowing `per_second` requests per second.
    ///
    /// # Panics
    /// Panics if `per_second` is 0.
    pub fn new(label: impl Into<String>, per_second: u32) -> Self {
        let nz = NonZeroU32::new(per_second).expect("per_second must be > 0");
        let quota = Quota::per_second(nz);
        Self {
            inner: Some(Arc::new(RateLimiter::direct(quota))),
            label: label.into(),
        }
    }

    /// Create a disabled/no-op rate limiter.
    ///
    /// Calls to [`acquire`](Self::acquire) return immediately.
    pub fn disabled(label: impl Into<String>) -> Self {
        Self {
            inner: None,
            label: label.into(),
        }
    }

    /// Create a new rate limiter from an exact `governor::Quota`.
    ///
    /// Use this when you need exact period-based spacing (e.g. RPM via
    /// `Quota::with_period(Duration::from_secs(60) / rpm)`) rather than the
    /// approximate integer `per_second` constructor.
    ///
    /// # Panics
    /// Panics if the quota burst size is zero (which should not happen in
    /// practice for well-formed `Quota` values).
    pub fn from_quota(label: impl Into<String>, quota: Quota) -> Self {
        Self {
            inner: Some(Arc::new(RateLimiter::direct(quota))),
            label: label.into(),
        }
    }

    /// Create a Finnhub rate limiter from `RateLimitConfig`.
    ///
    /// Returns `None` when `cfg.finnhub_rps == 0` (disabled).
    pub fn finnhub_from_config(cfg: &RateLimitConfig) -> Option<Self> {
        if cfg.finnhub_rps == 0 {
            return None;
        }
        Some(Self::new("finnhub", cfg.finnhub_rps))
    }

    /// Create a FRED rate limiter from `RateLimitConfig`.
    ///
    /// Returns `None` when `cfg.fred_rps == 0` (disabled).
    pub fn fred_from_config(cfg: &RateLimitConfig) -> Option<Self> {
        if cfg.fred_rps == 0 {
            return None;
        }
        Some(Self::new("fred", cfg.fred_rps))
    }

    /// Wait until a single permit becomes available. This is cancel-safe.
    pub async fn acquire(&self) {
        if let Some(inner) = &self.inner {
            inner.until_ready().await;
        }
    }

    /// The human-readable label for this limiter (e.g., provider name).
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// Per-provider LLM rate limiters keyed by [`ProviderId`].
///
/// Constructed from [`ProvidersConfig`] via [`ProviderRateLimiters::from_config`].
/// Providers with an RPM of `0` are absent from the internal map — callers
/// receive `None` from [`get`][Self::get] and skip the acquire step.
#[derive(Debug, Clone, Default)]
pub struct ProviderRateLimiters {
    limiters: HashMap<ProviderId, SharedRateLimiter>,
}

impl ProviderRateLimiters {
    /// Build a registry from `ProvidersConfig`.
    ///
    /// For each provider where `rpm > 0`, a `SharedRateLimiter` is created using
    /// `Quota::with_period(Duration::from_secs(60) / rpm)` for exact per-request
    /// spacing. Providers with `rpm == 0` are omitted.
    pub fn from_config(cfg: &ProvidersConfig) -> Self {
        let mut limiters = HashMap::new();

        let provider_rpms = [
            (ProviderId::OpenAI, cfg.openai.rpm, "openai"),
            (ProviderId::Anthropic, cfg.anthropic.rpm, "anthropic"),
            (ProviderId::Gemini, cfg.gemini.rpm, "gemini"),
            (ProviderId::Copilot, cfg.copilot.rpm, "copilot"),
            (ProviderId::OpenRouter, cfg.openrouter.rpm, "openrouter"),
        ];

        for (provider, rpm, label) in provider_rpms {
            if rpm > 0 {
                // Exact per-request spacing: divide 60-second window by rpm.
                // Using with_period avoids integer division loss (e.g. 30 RPM → 2s period).
                let period = Duration::from_secs(60) / rpm;
                let quota = Quota::with_period(period)
                    .expect("non-zero period should always produce a valid quota");
                limiters.insert(provider, SharedRateLimiter::from_quota(label, quota));
            }
        }

        Self { limiters }
    }

    /// Return the rate limiter for `provider`, or `None` if rate limiting is disabled
    /// for that provider.
    pub fn get(&self, provider: ProviderId) -> Option<&SharedRateLimiter> {
        self.limiters.get(&provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_issues_permits() {
        let limiter = SharedRateLimiter::new("test", 100);
        // Should not block for a single permit
        limiter.acquire().await;
        assert_eq!(limiter.label(), "test");
    }

    #[tokio::test]
    async fn disabled_limiter_is_noop() {
        let limiter = SharedRateLimiter::disabled("disabled-test");
        limiter.acquire().await;
        assert_eq!(limiter.label(), "disabled-test");
    }

    #[tokio::test]
    async fn from_quota_issues_permits() {
        let quota = Quota::with_period(Duration::from_millis(10)).expect("valid quota");
        let limiter = SharedRateLimiter::from_quota("test-quota", quota);
        limiter.acquire().await;
        assert_eq!(limiter.label(), "test-quota");
    }

    use crate::config::ProviderSettings;

    fn providers_config_with(overrides: &[(ProviderId, u32)]) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        for &(provider, rpm) in overrides {
            match provider {
                ProviderId::OpenAI => cfg.openai.rpm = rpm,
                ProviderId::Anthropic => cfg.anthropic.rpm = rpm,
                ProviderId::Gemini => cfg.gemini.rpm = rpm,
                ProviderId::Copilot => cfg.copilot.rpm = rpm,
                ProviderId::OpenRouter => cfg.openrouter.rpm = rpm,
            }
        }
        cfg
    }

    fn all_disabled_providers_config() -> ProvidersConfig {
        ProvidersConfig {
            openai: ProviderSettings {
                base_url: None,
                rpm: 0,
                ..Default::default()
            },
            anthropic: ProviderSettings {
                base_url: None,
                rpm: 0,
                ..Default::default()
            },
            gemini: ProviderSettings {
                base_url: None,
                rpm: 0,
                ..Default::default()
            },
            copilot: ProviderSettings {
                base_url: None,
                rpm: 0,
                ..Default::default()
            },
            openrouter: ProviderSettings {
                base_url: None,
                rpm: 0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn provider_rate_limiters_construction_mixed_rpms() {
        let mut cfg = all_disabled_providers_config();
        cfg.openai.rpm = 500;
        cfg.gemini.rpm = 60;
        cfg.openrouter.rpm = 20;
        let registry = ProviderRateLimiters::from_config(&cfg);

        // Enabled providers are present
        assert!(
            registry.get(ProviderId::OpenAI).is_some(),
            "openai should be enabled"
        );
        assert!(
            registry.get(ProviderId::Gemini).is_some(),
            "gemini should be enabled"
        );
        assert!(
            registry.get(ProviderId::OpenRouter).is_some(),
            "openrouter should be enabled"
        );

        // Disabled providers are absent
        assert!(
            registry.get(ProviderId::Anthropic).is_none(),
            "anthropic (rpm=0) should be absent"
        );
        assert!(
            registry.get(ProviderId::Copilot).is_none(),
            "copilot (rpm=0) should be absent"
        );
    }

    #[test]
    fn provider_rate_limiters_get_returns_some_for_enabled() {
        let cfg = providers_config_with(&[(ProviderId::OpenAI, 100)]);
        let registry = ProviderRateLimiters::from_config(&cfg);
        assert!(registry.get(ProviderId::OpenAI).is_some());
    }

    #[test]
    fn provider_rate_limiters_get_returns_some_for_custom_openrouter_rate() {
        let mut cfg = all_disabled_providers_config();
        cfg.openrouter.rpm = 100;
        let registry = ProviderRateLimiters::from_config(&cfg);
        assert!(registry.get(ProviderId::OpenRouter).is_some());
    }

    #[test]
    fn provider_rate_limiters_get_returns_none_for_disabled() {
        let cfg = all_disabled_providers_config();
        let registry = ProviderRateLimiters::from_config(&cfg);
        assert!(registry.get(ProviderId::OpenAI).is_none());
        assert!(registry.get(ProviderId::Anthropic).is_none());
        assert!(registry.get(ProviderId::Gemini).is_none());
        assert!(registry.get(ProviderId::Copilot).is_none());
        assert!(
            registry.get(ProviderId::OpenRouter).is_none(),
            "openrouter (rpm=0) should be absent"
        );
    }

    #[test]
    fn finnhub_from_config_returns_some_when_rps_nonzero() {
        let cfg = RateLimitConfig {
            finnhub_rps: 30,
            fred_rps: 0,
        };
        let limiter = SharedRateLimiter::finnhub_from_config(&cfg);
        assert!(limiter.is_some());
        assert_eq!(limiter.unwrap().label(), "finnhub");
    }

    #[test]
    fn finnhub_from_config_returns_none_when_rps_zero() {
        let cfg = RateLimitConfig {
            finnhub_rps: 0,
            fred_rps: 0,
        };
        let limiter = SharedRateLimiter::finnhub_from_config(&cfg);
        assert!(limiter.is_none());
    }

    #[test]
    fn fred_from_config_returns_some_when_rps_nonzero() {
        let cfg = RateLimitConfig {
            fred_rps: 5,
            ..Default::default()
        };
        let limiter = SharedRateLimiter::fred_from_config(&cfg);
        assert!(limiter.is_some());
    }

    #[test]
    fn fred_from_config_returns_none_when_rps_zero() {
        let cfg = RateLimitConfig {
            fred_rps: 0,
            ..Default::default()
        };
        let limiter = SharedRateLimiter::fred_from_config(&cfg);
        assert!(limiter.is_none());
    }
}
