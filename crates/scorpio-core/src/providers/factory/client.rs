//! Provider client construction and configuration validation.
//!
//! - [`CompletionModelHandle`] — reusable handle bundling provider, model ID, client, and rate limiter.
//! - [`create_completion_model`] — construct a handle from tier + config.

use rig::providers::{anthropic, deepseek, gemini, openai, openrouter};
use secrecy::ExposeSecret;
use tracing::info;

use crate::{
    config::{LlmConfig, ProviderSettings, ProvidersConfig},
    error::TradingError,
    providers::{ModelTier, ProviderId},
    rate_limit::{ProviderRateLimiters, SharedRateLimiter},
};

// ────────────────────────────────────────────────────────────────────────────
// CompletionModelHandle
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompletionModelHandle {
    provider: ProviderId,
    model_id: String,
    pub(super) client: ProviderClient,
    /// Rate limiter for this provider, or `None` if rate limiting is disabled.
    rate_limiter: Option<SharedRateLimiter>,
}

impl CompletionModelHandle {
    pub const fn provider_id(&self) -> ProviderId {
        self.provider
    }

    pub const fn provider_name(&self) -> &'static str {
        self.provider.as_str()
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Return the rate limiter for this provider, if one is configured.
    pub fn rate_limiter(&self) -> Option<&SharedRateLimiter> {
        self.rate_limiter.as_ref()
    }

    /// Construct a non-functional handle for use in tests only.
    ///
    /// The resulting handle has a real `OpenAI` client built with a dummy key.
    /// Any LLM call made through this handle will fail with an auth error,
    /// which is intentional: tests use the error to prove the underlying agent
    /// function was actually called (rather than being a silent no-op).
    ///
    /// # Note
    ///
    /// This method is public to allow integration tests in `tests/` to access
    /// it.  It must not be called in production code.
    #[cfg(any(test, feature = "test-helpers"))]
    #[doc(hidden)]
    pub fn for_test() -> Self {
        Self {
            provider: ProviderId::OpenAI,
            model_id: "test-model".to_owned(),
            client: ProviderClient::OpenAI(
                openai::Client::new("test-dummy-key").expect("openai client construction"),
            ),
            rate_limiter: None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// ProviderClient enum
// ────────────────────────────────────────────────────────────────────────────

/// A provider-agnostic client wrapping the concrete `rig` provider clients.
///
/// Because `rig`'s `CompletionModel` trait is not dyn-compatible (uses `impl Future`
/// returns and requires `Clone`), we use enum dispatch to support multiple providers
/// behind a single type.
#[derive(Debug, Clone)]
pub(crate) enum ProviderClient {
    /// OpenAI Responses API client (default for OpenAI).
    OpenAI(openai::Client),
    /// Anthropic Messages API client.
    Anthropic(anthropic::Client),
    /// Google Gemini API client.
    Gemini(gemini::Client),
    /// OpenRouter API aggregator (300+ models, including free-tier).
    OpenRouter(openrouter::Client),
    /// DeepSeek API (deepseek-chat, deepseek-reasoner).
    DeepSeek(deepseek::Client),
}

// ────────────────────────────────────────────────────────────────────────────
// Factory functions
// ────────────────────────────────────────────────────────────────────────────

/// Construct a reusable completion-model handle from configuration.
///
/// Resolves provider from the requested `tier`, then extracts the
/// corresponding API key from `api_config`. Returns `TradingError::Config` for unknown
/// providers, invalid model IDs, or missing keys.
///
/// Per-provider `base_url` overrides are read from `providers_config`. When a provider
/// has a custom base URL configured, the client is constructed via the builder pattern
/// (`Client::builder().api_key(key).base_url(url).build()`) instead of `Client::new(key)`.
///
/// The `rate_limiters` registry is used to attach a per-provider rate limiter to the
/// handle. Pass `&ProviderRateLimiters::default()` to disable rate limiting.
pub fn create_completion_model(
    tier: ModelTier,
    llm_config: &LlmConfig,
    providers_config: &ProvidersConfig,
    rate_limiters: &ProviderRateLimiters,
) -> Result<CompletionModelHandle, TradingError> {
    let provider = validate_provider_id(tier.provider_id(llm_config))?;
    let model_id = validate_model_id(tier.model_id(llm_config))?;
    let settings = providers_config.settings_for(provider);
    let client = create_provider_client_for(provider, settings, &model_id)?;
    let rate_limiter = rate_limiters.get(provider).cloned();
    info!(provider = provider.as_str(), model = model_id.as_str(), tier = %tier, "LLM completion model handle created");
    Ok(CompletionModelHandle {
        provider,
        model_id,
        client,
        rate_limiter,
    })
}

fn create_provider_client_for(
    provider: ProviderId,
    settings: &ProviderSettings,
    _model_id: &str,
) -> Result<ProviderClient, TradingError> {
    let base_url = settings.base_url.as_deref();
    match provider {
        ProviderId::OpenAI => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match base_url {
                Some(url) => openai::Client::builder()
                    .api_key(key.expose_secret())
                    .base_url(url)
                    .build()
                    .map_err(|e| {
                        config_error(&format!(
                            "failed to create OpenAI client with base_url \"{url}\": {e}"
                        ))
                    })?,
                None => openai::Client::new(key.expose_secret())
                    .map_err(|e| config_error(&format!("failed to create OpenAI client: {e}")))?,
            };
            Ok(ProviderClient::OpenAI(client))
        }
        ProviderId::Anthropic => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match base_url {
                Some(url) => anthropic::Client::builder()
                    .api_key(key.expose_secret())
                    .base_url(url)
                    .build()
                    .map_err(|e| {
                        config_error(&format!(
                            "failed to create Anthropic client with base_url \"{url}\": {e}"
                        ))
                    })?,
                None => anthropic::Client::new(key.expose_secret()).map_err(|e| {
                    config_error(&format!("failed to create Anthropic client: {e}"))
                })?,
            };
            Ok(ProviderClient::Anthropic(client))
        }
        ProviderId::Gemini => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match base_url {
                Some(url) => gemini::Client::builder()
                    .api_key(key.expose_secret())
                    .base_url(url)
                    .build()
                    .map_err(|e| {
                        config_error(&format!(
                            "failed to create Gemini client with base_url \"{url}\": {e}"
                        ))
                    })?,
                None => gemini::Client::new(key.expose_secret())
                    .map_err(|e| config_error(&format!("failed to create Gemini client: {e}")))?,
            };
            Ok(ProviderClient::Gemini(client))
        }
        ProviderId::OpenRouter => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match base_url {
                Some(url) => openrouter::Client::builder()
                    .api_key(key.expose_secret())
                    .base_url(url)
                    .build()
                    .map_err(|e| {
                        config_error(&format!(
                            "failed to create OpenRouter client with base_url \"{url}\": {e}"
                        ))
                    })?,
                None => openrouter::Client::new(key.expose_secret()).map_err(|e| {
                    config_error(&format!("failed to create OpenRouter client: {e}"))
                })?,
            };
            Ok(ProviderClient::OpenRouter(client))
        }
        ProviderId::DeepSeek => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match base_url {
                Some(url) => deepseek::Client::builder()
                    .api_key(key.expose_secret())
                    .base_url(url)
                    .build()
                    .map_err(|e| {
                        config_error(&format!(
                            "failed to create DeepSeek client with base_url \"{url}\": {e}"
                        ))
                    })?,
                None => deepseek::Client::new(key.expose_secret())
                    .map_err(|e| config_error(&format!("failed to create DeepSeek client: {e}")))?,
            };
            Ok(ProviderClient::DeepSeek(client))
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Validation helpers
// ────────────────────────────────────────────────────────────────────────────

fn validate_provider_id(provider: &str) -> Result<ProviderId, TradingError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(ProviderId::OpenAI),
        "anthropic" => Ok(ProviderId::Anthropic),
        "gemini" => Ok(ProviderId::Gemini),
        "openrouter" => Ok(ProviderId::OpenRouter),
        "deepseek" => Ok(ProviderId::DeepSeek),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek)"
        ))),
    }
}

fn validate_model_id(model_id: &str) -> Result<String, TradingError> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err(config_error("LLM model ID must not be empty"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(config_error(
            "LLM model ID must not contain control characters",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Convenience for creating `TradingError::Config` from a message.
fn config_error(msg: &str) -> TradingError {
    TradingError::Config(anyhow::anyhow!("{}", msg))
}

/// Convenience for creating a missing-API-key config error.
fn missing_key_error(provider: ProviderId) -> TradingError {
    config_error(&format!(
        "API key for provider \"{}\" is not set (expected env var: {})",
        provider.as_str(),
        provider.missing_key_hint()
    ))
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, ProviderSettings, ProvidersConfig};
    use secrecy::SecretString;

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    fn providers_config_with_openai() -> ProvidersConfig {
        ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn providers_config_with_anthropic() -> ProvidersConfig {
        ProvidersConfig {
            anthropic: ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn providers_config_with_gemini() -> ProvidersConfig {
        ProvidersConfig {
            gemini: ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn providers_config_with_openrouter() -> ProvidersConfig {
        ProvidersConfig {
            openrouter: ProviderSettings {
                api_key: Some(SecretString::from("test-openrouter-key")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn providers_config_with_deepseek() -> ProvidersConfig {
        ProvidersConfig {
            deepseek: ProviderSettings {
                api_key: Some(SecretString::from("test-deepseek-key")),
                base_url: None,
                rpm: 60,
            },
            ..ProvidersConfig::default()
        }
    }

    // ── Factory error paths ──────────────────────────────────────────────

    #[test]
    fn factory_unknown_provider_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "unsupported".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown LLM provider"),
            "expected 'unknown LLM provider' in: {msg}"
        );
    }

    #[test]
    fn factory_missing_openai_key_returns_config_error() {
        let cfg = sample_llm_config();
        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("SCORPIO_OPENAI_API_KEY"),
            "expected env var hint in: {msg}"
        );
    }

    #[test]
    fn factory_missing_anthropic_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "anthropic".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("SCORPIO_ANTHROPIC_API_KEY"),
            "expected env var hint in: {msg}"
        );
    }

    #[test]
    fn factory_missing_gemini_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "gemini".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("SCORPIO_GEMINI_API_KEY"),
            "expected env var hint in: {msg}"
        );
    }

    // ── Factory success paths ────────────────────────────────────────────

    #[test]
    fn factory_creates_openai_client() {
        let cfg = sample_llm_config();
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openai(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "openai");
        assert_eq!(handle.model_id(), "gpt-4o-mini");
        assert!(matches!(handle.client, ProviderClient::OpenAI(_)));
    }

    #[test]
    fn factory_creates_anthropic_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_anthropic(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "anthropic");
        assert_eq!(handle.model_id(), "o3");
        assert!(matches!(handle.client, ProviderClient::Anthropic(_)));
    }

    #[test]
    fn factory_creates_gemini_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_gemini(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "gemini");
        assert_eq!(handle.model_id(), "o3");
        assert!(matches!(handle.client, ProviderClient::Gemini(_)));
    }

    #[test]
    fn factory_empty_model_id_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_model = "   ".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openai(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("model ID"));
    }

    // ── OpenRouter provider ──────────────────────────────────────────────

    #[test]
    fn validate_provider_id_openrouter_returns_openrouter() {
        let result = validate_provider_id("openrouter");
        assert!(
            result.is_ok(),
            "\"openrouter\" should be a valid provider id: {result:?}"
        );
        assert_eq!(result.unwrap(), ProviderId::OpenRouter);
    }

    #[test]
    fn validate_provider_id_openrouter_normalises_case_and_whitespace() {
        let result = validate_provider_id("  OpenRouter  ");
        assert_eq!(result.unwrap(), ProviderId::OpenRouter);
    }

    #[test]
    fn validate_provider_id_unknown_error_lists_openrouter() {
        let result = validate_provider_id("unknown-provider");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("openrouter"), "expected openrouter in: {msg}");
    }

    #[test]
    fn factory_missing_openrouter_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3.6-plus-preview:free".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("SCORPIO_OPENROUTER_API_KEY"),
            "expected env var hint in: {msg}"
        );
    }

    #[test]
    fn factory_creates_openrouter_client() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3.6-plus-preview:free".to_owned();

        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok(), "OpenRouter client creation should succeed");
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "openrouter");
        assert_eq!(handle.model_id(), "qwen/qwen3.6-plus-preview:free");
        assert!(matches!(handle.client, ProviderClient::OpenRouter(_)));
    }

    #[test]
    fn factory_creates_openrouter_client_for_deep_thinking_tier() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "openrouter".to_owned();
        cfg.deep_thinking_model = "minimax/minimax-m2.5:free".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();

        assert_eq!(handle.provider_name(), "openrouter");
        assert_eq!(handle.model_id(), "minimax/minimax-m2.5:free");
        assert!(matches!(handle.client, ProviderClient::OpenRouter(_)));
    }

    #[test]
    fn openrouter_free_model_identifiers_accepted_unchanged() {
        // Free-model identifiers include slashes and `:free` suffixes — they must
        // pass through `validate_model_id` unmodified (only empty/whitespace-only
        // values are rejected).
        for model in &[
            "qwen/qwen3.6-plus-preview:free",
            "minimax/minimax-m2.5:free",
        ] {
            let mut cfg = sample_llm_config();
            cfg.quick_thinking_provider = "openrouter".to_owned();
            cfg.quick_thinking_model = model.to_string();

            let handle = create_completion_model(
                ModelTier::QuickThinking,
                &cfg,
                &providers_config_with_openrouter(),
                &ProviderRateLimiters::default(),
            );
            assert!(
                handle.is_ok(),
                "free-model identifier '{model}' should be accepted: {handle:?}"
            );
            assert_eq!(
                handle.unwrap().model_id(),
                *model,
                "model id should be passed through unchanged"
            );
        }
    }

    #[test]
    fn openrouter_model_id_with_control_chars_is_rejected() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3\n:free".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("control characters"),
            "expected control-character validation error"
        );
    }

    // ── Copilot removal test ───────────────────────────────────────────────

    #[test]
    fn validate_provider_id_rejects_copilot() {
        let err = validate_provider_id("copilot").expect_err("copilot should be rejected");
        let msg = err.to_string();
        assert!(msg.contains("copilot"));
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("deepseek"));
    }

    // ── DeepSeek provider tests ──────────────────────────────────────────────

    #[test]
    fn validate_provider_id_deepseek_returns_deepseek() {
        let result = validate_provider_id("deepseek");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderId::DeepSeek);
    }

    #[test]
    fn factory_missing_deepseek_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "deepseek".to_owned();
        cfg.quick_thinking_model = "deepseek-chat".to_owned();
        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("SCORPIO_DEEPSEEK_API_KEY"),
            "expected env var hint in: {msg}"
        );
    }

    #[test]
    fn factory_creates_deepseek_client() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "deepseek".to_owned();
        cfg.quick_thinking_model = "deepseek-chat".to_owned();
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_deepseek(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "deepseek");
        assert_eq!(handle.model_id(), "deepseek-chat");
        assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
    }

    #[test]
    fn create_completion_model_attaches_deepseek_rate_limiter() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "deepseek".to_owned();
        cfg.quick_thinking_model = "deepseek-chat".to_owned();
        let providers = ProvidersConfig {
            deepseek: ProviderSettings {
                api_key: Some(SecretString::from("test-deepseek-key")),
                base_url: None,
                rpm: 75,
            },
            ..ProvidersConfig::default()
        };
        let limiters = ProviderRateLimiters::from_config(&providers);
        let handle =
            create_completion_model(ModelTier::QuickThinking, &cfg, &providers, &limiters).unwrap();
        assert_eq!(handle.rate_limiter().map(|l| l.label()), Some("deepseek"));
    }

    #[test]
    fn factory_creates_deepseek_client_with_base_url_override() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "deepseek".to_owned();
        cfg.quick_thinking_model = "deepseek-chat".to_owned();
        let providers = ProvidersConfig {
            deepseek: ProviderSettings {
                api_key: Some(SecretString::from("test-deepseek-key")),
                base_url: Some("https://deepseek.example.com/v1".to_owned()),
                rpm: 60,
            },
            ..ProvidersConfig::default()
        };
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "deepseek");
        assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
    }

    #[test]
    fn factory_creates_deepseek_client_for_deep_thinking_tier() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "deepseek".to_owned();
        cfg.deep_thinking_model = "deepseek-reasoner".to_owned();
        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_deepseek(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "deepseek");
        assert_eq!(handle.model_id(), "deepseek-reasoner");
        assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
    }
}
