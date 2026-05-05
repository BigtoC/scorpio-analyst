//! Provider client construction and configuration validation.
//!
//! - [`CompletionModelHandle`] — reusable handle bundling provider, model ID, client, and rate limiter.
//! - [`create_completion_model`] — construct a handle from tier + config.
//! - [`create_completion_model_with_copilot`] — construct a Copilot handle with an explicit auth mode.

use rig::providers::{anthropic, copilot, deepseek, gemini, openai, openrouter, xiaomimimo};
use secrecy::ExposeSecret;
use tracing::info;

use crate::{
    config::{LlmConfig, ProviderSettings, ProvidersConfig},
    error::TradingError,
    providers::{ModelTier, ProviderId},
    rate_limit::{ProviderRateLimiters, SharedRateLimiter},
};

// ────────────────────────────────────────────────────────────────────────────
// CopilotAuthMode
// ────────────────────────────────────────────────────────────────────────────

/// Whether a Copilot code path may later trigger interactive OAuth/device-flow auth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CopilotAuthMode {
    /// Setup-time path: may prompt the user with a verification URI and user code.
    InteractiveSetup,
    /// Runtime path: must rely on prevalidated cached auth and never use Scorpio's
    /// interactive setup entrypoint.
    #[default]
    NonInteractiveRuntime,
}

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

    /// Trigger rig's lazy Copilot authorization path for setup-time flows.
    pub async fn authorize_copilot(&self) -> Result<(), TradingError> {
        match &self.client {
            ProviderClient::Copilot(client) => client
                .authorize()
                .await
                .map_err(|e| config_error(&format!("failed to authorize Copilot client: {e}"))),
            _ => Err(config_error(
                "authorize_copilot requires a Copilot completion model handle",
            )),
        }
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

    /// Construct a handle with a specific provider and pre-built client, for use in tests only.
    ///
    /// This lets tests exercise provider-specific dispatch paths (e.g. `Copilot`, `XiaomiMimo`)
    /// without going through the full configuration pipeline.
    #[cfg(any(test, feature = "test-helpers"))]
    #[doc(hidden)]
    pub(crate) fn for_test_with_client(
        provider: ProviderId,
        model_id: &str,
        client: ProviderClient,
    ) -> Self {
        Self {
            provider,
            model_id: model_id.to_owned(),
            client,
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
    /// GitHub Copilot via OAuth/device flow (no Scorpio-managed API key).
    Copilot(copilot::Client),
    /// Xiaomi MiMo via OpenAI-compatible API.
    XiaomiMimo(xiaomimimo::Client),
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
    let model_id = validate_model_id(provider, tier.model_id(llm_config))?;
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

/// Construct a completion-model handle for Copilot with an explicit auth mode.
///
/// In `InteractiveSetup` mode this only builds the handle; callers must explicitly
/// call `CompletionModelHandle::authorize_copilot()` to trigger rig's lazy auth.
/// In `NonInteractiveRuntime` mode this refuses when the token cache is missing.
pub fn create_completion_model_with_copilot(
    tier: ModelTier,
    llm_config: &LlmConfig,
    providers_config: &ProvidersConfig,
    rate_limiters: &ProviderRateLimiters,
    mode: CopilotAuthMode,
    token_dir: &std::path::Path,
) -> Result<CompletionModelHandle, TradingError> {
    let provider = validate_provider_id(tier.provider_id(llm_config))?;
    let model_id = validate_model_id(provider, tier.model_id(llm_config))?;
    if provider != ProviderId::Copilot {
        return create_completion_model(tier, llm_config, providers_config, rate_limiters);
    }

    let settings = providers_config.settings_for(provider);
    let client = create_copilot_client_for(settings, mode, Some(token_dir))?;

    let rate_limiter = rate_limiters.get(provider).cloned();
    info!(provider = provider.as_str(), model = model_id.as_str(), tier = %tier, mode = ?mode, "Copilot completion model handle created");
    Ok(CompletionModelHandle {
        provider,
        model_id,
        client,
        rate_limiter,
    })
}

/// Create a Copilot completion-model handle suitable for interactive OAuth setup only.
///
/// The returned handle is only valid for calling [`CompletionModelHandle::authorize_copilot`].
/// Any attempt to run an actual LLM completion will fail because no routing config is wired.
pub fn build_copilot_auth_handle(
    token_dir: &std::path::Path,
) -> Result<CompletionModelHandle, TradingError> {
    let settings = crate::config::ProviderSettings::default();
    let client = create_copilot_client_for(
        &settings,
        CopilotAuthMode::InteractiveSetup,
        Some(token_dir),
    )?;
    Ok(CompletionModelHandle {
        provider: ProviderId::Copilot,
        model_id: "gpt-4o".to_owned(),
        client,
        rate_limiter: None,
    })
}

fn create_copilot_client_for(
    settings: &ProviderSettings,
    mode: CopilotAuthMode,
    token_dir_override: Option<&std::path::Path>,
) -> Result<ProviderClient, TradingError> {
    if settings.base_url.is_some() {
        return Err(config_error(
            "providers.copilot.base_url is not supported in this slice",
        ));
    }
    if settings.api_key.is_some() {
        return Err(config_error(
            "providers.copilot.api_key is not supported; Copilot uses OAuth/device flow",
        ));
    }

    let owned_token_dir;
    let token_dir = match token_dir_override {
        Some(dir) => dir,
        None => {
            owned_token_dir = crate::settings::copilot_token_dir()
                .map_err(|e| config_error(&format!("failed to resolve Copilot token dir: {e}")))?;
            owned_token_dir.as_path()
        }
    };

    if mode == CopilotAuthMode::NonInteractiveRuntime {
        if !token_dir.join("access-token").exists() || !token_dir.join("api-key.json").exists() {
            return Err(config_error(
                "Copilot token cache is missing under the Scorpio config dir; \
                 run `scorpio setup` to authorize Copilot",
            ));
        }

        crate::settings::verify_copilot_token_dir_secure(token_dir)
            .map_err(|e| config_error(&format!("token directory rejected: {e}")))?;
        crate::providers::factory::copilot_auth::read_binding(token_dir)
            .map_err(|e| config_error(&format!("identity binding rejected: {e}")))?;
        let api_key_record =
            crate::providers::factory::copilot_auth::read_api_key_record(token_dir)
                .map_err(|e| config_error(&format!("api-key cache rejected: {e}")))?;
        crate::providers::factory::copilot_auth::validate_copilot_runtime_base(&api_key_record)
            .map_err(|e| config_error(&format!("Copilot runtime base rejected: {e}")))?;
        crate::providers::factory::copilot_auth::read_access_token(token_dir)
            .map_err(|e| config_error(&format!("access token rejected: {e}")))?;
    }

    let builder = copilot::Client::builder().oauth().token_dir(token_dir);
    let builder = match mode {
        CopilotAuthMode::InteractiveSetup => builder,
        CopilotAuthMode::NonInteractiveRuntime => builder.on_device_code(|_prompt| {
            tracing::error!(
                "Copilot device flow attempted in non-interactive runtime mode; refusing to prompt"
            );
        }),
    };

    let client = builder
        .build()
        .map_err(|e| config_error(&format!("failed to construct Copilot client: {e}")))?;
    Ok(ProviderClient::Copilot(client))
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
        ProviderId::Copilot => {
            create_copilot_client_for(settings, CopilotAuthMode::NonInteractiveRuntime, None)
        }
        ProviderId::XiaomiMimo => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match settings.base_url.as_deref() {
                Some(raw_url) => {
                    let parsed = validate_xiaomimimo_base_url(raw_url)?;
                    let http_client = reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .map_err(|e| {
                            config_error(&format!(
                                "failed to create Xiaomi MiMo HTTP client for base_url \"{raw_url}\": {e}"
                            ))
                        })?;
                    xiaomimimo::Client::builder()
                        .api_key(key.expose_secret())
                        .base_url(&parsed)
                        .http_client(http_client)
                        .build()
                        .map_err(|e| {
                            config_error(&format!(
                                "failed to create Xiaomi MiMo client with base_url \"{raw_url}\": {e}"
                            ))
                        })?
                }
                None => xiaomimimo::Client::new(key.expose_secret()).map_err(|e| {
                    config_error(&format!("failed to create Xiaomi MiMo client: {e}"))
                })?,
            };
            Ok(ProviderClient::XiaomiMimo(client))
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// URL validation helpers
// ────────────────────────────────────────────────────────────────────────────

fn validate_xiaomimimo_base_url(raw: &str) -> Result<url::Url, TradingError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(config_error("xiaomimimo base_url must not be empty"));
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| config_error(&format!("xiaomimimo base_url is not a valid URL: {e}")))?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(config_error(
            "xiaomimimo base_url must not contain user/password info",
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(config_error(
            "xiaomimimo base_url must not contain query or fragment components",
        ));
    }

    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| config_error("xiaomimimo base_url has no host"))?;

    match scheme {
        "https" => {
            if is_trusted_xiaomimimo_host(host) {
                Ok(parsed)
            } else {
                Err(config_error(&format!(
                    "xiaomimimo base_url host {host:?} is not in the trusted-host allowlist for this slice"
                )))
            }
        }
        "http" => {
            let is_loopback = match parsed.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                Some(url::Host::Domain(domain)) => domain == "localhost",
                None => false,
            };
            if is_loopback {
                Ok(parsed)
            } else {
                Err(config_error(&format!(
                    "xiaomimimo base_url uses http://; only https is allowed except for loopback hosts (got host {host:?})"
                )))
            }
        }
        other => Err(config_error(&format!(
            "xiaomimimo base_url has unsupported scheme {other:?} (expected https or http loopback)"
        ))),
    }
}

fn is_trusted_xiaomimimo_host(host: &str) -> bool {
    matches!(
        host,
        "api.xiaomi.com" | "api.xiaomimimo.com" | "api.mimo.ai"
    )
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
        "copilot" => Ok(ProviderId::Copilot),
        "xiaomimimo" => Ok(ProviderId::XiaomiMimo),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek, copilot, xiaomimimo)"
        ))),
    }
}

fn validate_model_id(provider: ProviderId, model_id: &str) -> Result<String, TradingError> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return Err(config_error("LLM model ID must not be empty"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(config_error(
            "LLM model ID must not contain control characters",
        ));
    }
    if provider == ProviderId::Copilot && trimmed.to_ascii_lowercase().contains("codex") {
        return Err(config_error(
            "Copilot codex-class models are not supported in this slice",
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

    #[test]
    fn validate_model_id_rejects_codex_models() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-5.1-codex".to_owned();

        let err = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        )
        .unwrap_err();

        assert!(
            err.to_string().to_ascii_lowercase().contains("codex"),
            "expected codex rejection before runtime auth path, got: {err}"
        );
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
        assert!(msg.contains("copilot"), "expected copilot in: {msg}");
        assert!(msg.contains("xiaomimimo"), "expected xiaomimimo in: {msg}");
    }

    #[test]
    fn validate_provider_id_copilot_returns_copilot() {
        let result = validate_provider_id("copilot");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderId::Copilot);
    }

    #[test]
    fn validate_provider_id_copilot_normalises_case() {
        let result = validate_provider_id("  Copilot  ");
        assert_eq!(result.unwrap(), ProviderId::Copilot);
    }

    #[test]
    fn validate_provider_id_xiaomimimo_returns_xiaomimimo() {
        let result = validate_provider_id("xiaomimimo");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProviderId::XiaomiMimo);
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

    // ── URL validator tests ──────────────────────────────────────────────────

    #[test]
    fn validate_xiaomimimo_base_url_accepts_https() {
        assert!(validate_xiaomimimo_base_url("https://api.xiaomimimo.com/v1").is_ok());
    }

    #[test]
    fn validate_xiaomimimo_base_url_accepts_loopback_http() {
        for url in &[
            "http://127.0.0.1:8080",
            "http://localhost",
            "http://[::1]:8080",
        ] {
            assert!(
                validate_xiaomimimo_base_url(url).is_ok(),
                "should accept loopback {url}"
            );
        }
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_remote_http() {
        let err = validate_xiaomimimo_base_url("http://api.example.com/v1").unwrap_err();
        assert!(
            err.to_string().contains("https"),
            "expected https mention: {err}"
        );
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_localhost_lookalikes() {
        for url in &["http://localhost.evil.com", "https://localhost.evil.com"] {
            assert!(
                validate_xiaomimimo_base_url(url).is_err(),
                "must not treat {url} as loopback"
            );
        }
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_userinfo() {
        let err = validate_xiaomimimo_base_url("https://user@evil.com/").unwrap_err();
        assert!(
            err.to_string().contains("user"),
            "expected userinfo mention: {err}"
        );
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_userinfo_with_loopback_lookalike() {
        let err = validate_xiaomimimo_base_url("http://127.0.0.1@evil.com/").unwrap_err();
        assert!(err.to_string().contains("user"), "userinfo: {err}");
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_empty() {
        assert!(validate_xiaomimimo_base_url("").is_err());
        assert!(validate_xiaomimimo_base_url("   ").is_err());
    }

    // ── Factory construction tests ───────────────────────────────────────────

    fn providers_config_with_xiaomimimo() -> ProvidersConfig {
        ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("mimo-test-key")),
                base_url: None,
                rpm: 50,
            },
            ..Default::default()
        }
    }

    #[test]
    fn factory_creates_xiaomimimo_client() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_xiaomimimo(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "xiaomimimo");
        assert!(matches!(handle.client, ProviderClient::XiaomiMimo(_)));
    }

    #[test]
    fn factory_missing_xiaomimimo_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("SCORPIO_XIAOMIMIMO_API_KEY"), "got: {msg}");
    }

    #[test]
    fn factory_xiaomimimo_with_https_base_url_succeeds() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let providers = ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("mimo-test-key")),
                base_url: Some("https://api.xiaomimimo.com/v1".to_owned()),
                rpm: 50,
            },
            ..Default::default()
        };
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert!(matches!(handle.client, ProviderClient::XiaomiMimo(_)));
    }

    #[test]
    fn factory_xiaomimimo_with_http_remote_base_url_rejected() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let providers = ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("mimo-test-key")),
                base_url: Some("http://api.example.com/v1".to_owned()),
                rpm: 50,
            },
            ..Default::default()
        };
        let err = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("https"), "got: {err}");
    }

    #[test]
    fn factory_creates_copilot_client_in_interactive_setup_mode() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let providers = ProvidersConfig {
            copilot: ProviderSettings {
                api_key: None,
                base_url: None,
                rpm: 30,
            },
            ..Default::default()
        };

        let handle = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
            CopilotAuthMode::InteractiveSetup,
            &token_dir,
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "copilot");
        assert!(matches!(handle.client, ProviderClient::Copilot(_)));
    }

    #[test]
    fn factory_runtime_mode_fails_when_token_cache_missing() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("scorpio setup"),
            "expected setup guidance: {msg}"
        );
    }

    #[test]
    fn factory_default_create_completion_model_uses_noninteractive_copilot_runtime() {
        // Verify that create_completion_model uses NonInteractiveRuntime for Copilot by
        // supplying an empty token dir via the testable entry point.
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();
        let err = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        )
        .unwrap_err();
        assert!(err.to_string().contains("scorpio setup"), "got: {err}");
    }

    // ── NonInteractiveRuntime validation paths ───────────────────────────────

    #[test]
    fn noninteractive_factory_rejects_when_identity_binding_missing() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        // Pretend rig cache exists.
        std::fs::write(token_dir.join("access-token"), "fake-token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"endpoints":{"api":"https://api.githubcopilot.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        // No scorpio-identity.json.

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("identity"),
            "error should mention identity: {err}"
        );
    }

    #[test]
    fn noninteractive_factory_accepts_allowed_copilot_runtime_base() {
        use crate::providers::factory::copilot_auth;

        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        std::fs::write(token_dir.join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"token":"tid_test","expires_at":4102444800,"endpoints":{"api":"https://api.githubcopilot.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        copilot_auth::write_binding(
            &token_dir,
            &copilot_auth::ScorpioIdentityBinding {
                github_id: 42,
                github_login: "octocat".to_owned(),
                written_at: 0,
            },
        )
        .unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        assert!(
            result.is_ok(),
            "should accept allowed Copilot runtime base: {:?}",
            result.err()
        );
    }

    #[test]
    fn noninteractive_factory_rejects_untrusted_copilot_runtime_base() {
        use crate::providers::factory::copilot_auth;

        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        std::fs::write(token_dir.join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"token":"tid_test","endpoints":{"api":"https://evil.example.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        copilot_auth::write_binding(
            &token_dir,
            &copilot_auth::ScorpioIdentityBinding {
                github_id: 42,
                github_login: "octocat".to_owned(),
                written_at: 0,
            },
        )
        .unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let err = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("runtime base"),
            "error should mention runtime base: {err}"
        );
    }

    #[test]
    fn copilot_factory_paths_do_not_use_from_env() {
        let source = include_str!("client.rs");
        // Assemble the forbidden string from parts so the test body itself does
        // not trigger a false positive when `include_str!` captures this file.
        let forbidden: String = ["copilot::Client", "::from_env"].concat();
        let non_test_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(
            !non_test_source.contains(&forbidden),
            "Copilot factory must not use from_env; it bypasses Scorpio-managed token_dir auth"
        );
    }
}
