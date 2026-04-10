//! Provider client construction and configuration validation.
//!
//! - [`CompletionModelHandle`] — reusable handle bundling provider, model ID, client, and rate limiter.
//! - [`create_completion_model`] — construct a handle from tier + config.
//! - [`preflight_copilot_if_configured`] — validate configured Copilot providers before the pipeline starts.

use rig::providers::{anthropic, gemini, openai, openrouter};
use secrecy::ExposeSecret;
use std::path::Path;
use tracing::info;

use crate::{
    config::{LlmConfig, ProviderSettings, ProvidersConfig},
    error::TradingError,
    providers::{ModelTier, ProviderId, copilot::CopilotProviderClient},
    rate_limit::{ProviderRateLimiters, SharedRateLimiter},
};

use super::error::sanitize_error_summary;

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
    /// GitHub Copilot via ACP (local CLI subprocess, no API key).
    Copilot(CopilotProviderClient),
    /// OpenRouter API aggregator (300+ models, including free-tier).
    OpenRouter(openrouter::Client),
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

pub async fn preflight_copilot_if_configured(
    llm_config: &LlmConfig,
    providers_config: &ProvidersConfig,
    rate_limiters: &ProviderRateLimiters,
) -> Result<(), TradingError> {
    for tier in [ModelTier::QuickThinking, ModelTier::DeepThinking] {
        if !matches!(
            validate_provider_id(tier.provider_id(llm_config)),
            Ok(ProviderId::Copilot)
        ) {
            continue;
        }

        let handle = create_completion_model(tier, llm_config, providers_config, rate_limiters)?;
        if let ProviderClient::Copilot(client) = &handle.client {
            client.preflight().await.map_err(|err| {
                TradingError::Rig(format!(
                    "provider=copilot model={} summary={}",
                    handle.model_id(),
                    sanitize_error_summary(&err.to_string())
                ))
            })?;
        }
    }

    Ok(())
}

fn create_provider_client_for(
    provider: ProviderId,
    settings: &ProviderSettings,
    model_id: &str,
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
        ProviderId::Copilot => {
            let exe_path = resolve_copilot_exe_path()?;
            validate_copilot_cli_path(&exe_path)?;
            Ok(ProviderClient::Copilot(CopilotProviderClient::new(
                exe_path, model_id,
            )))
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
        "copilot" => Ok(ProviderId::Copilot),
        "openrouter" => Ok(ProviderId::OpenRouter),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, copilot, openrouter)"
        ))),
    }
}

fn resolve_copilot_exe_path() -> Result<String, TradingError> {
    std::env::var("SCORPIO_COPILOT_CLI_PATH")
        .map_err(|_| {
            config_error(
                "SCORPIO_COPILOT_CLI_PATH must be set to the absolute path of the Copilot CLI executable. Scorpio no longer falls back to PATH lookup; install the Copilot CLI and set SCORPIO_COPILOT_CLI_PATH=/absolute/path/to/copilot.",
            )
        })
        .and_then(|path| resolve_copilot_exe_path_from(&path))
}

fn resolve_copilot_exe_path_from(path: &str) -> Result<String, TradingError> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(config_error("SCORPIO_COPILOT_CLI_PATH must not be empty"));
    }
    Ok(trimmed.to_owned())
}

/// Validate the Copilot CLI executable path.
///
/// Rejects paths that:
/// - Contain shell metacharacters that could enable injection.
/// - Contain `..` path-traversal sequences.
/// - Are not absolute filesystem paths.
/// - Do not exist, are not regular files, or are not executable.
fn validate_copilot_cli_path(path: &str) -> Result<(), TradingError> {
    const FORBIDDEN_CHARS: &[char] = &[
        ';', '|', '&', '$', '`', '(', ')', '<', '>', '"', '\'', '\n', '\r', '\0', '*', '?', '[',
        ']', '{', '}',
    ];

    if path.is_empty() {
        return Err(config_error("SCORPIO_COPILOT_CLI_PATH must not be empty"));
    }
    if path.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
        return Err(config_error(
            "SCORPIO_COPILOT_CLI_PATH contains disallowed characters",
        ));
    }
    if path.contains("..") {
        return Err(config_error(
            "SCORPIO_COPILOT_CLI_PATH must not contain path traversal (..)",
        ));
    }
    if !path.starts_with('/') {
        return Err(config_error(
            "SCORPIO_COPILOT_CLI_PATH must be an absolute path to an executable file. Plain command names are no longer accepted because Scorpio no longer falls back to PATH lookup.",
        ));
    }

    let cli_path = Path::new(path);
    if !cli_path.exists() {
        return Err(config_error(&format!(
            "SCORPIO_COPILOT_CLI_PATH points to '{path}', but that file does not exist. Install the Copilot CLI and set SCORPIO_COPILOT_CLI_PATH to its absolute path."
        )));
    }

    let metadata = std::fs::metadata(cli_path).map_err(|err| {
        config_error(&format!(
            "failed to read Copilot CLI metadata at '{path}': {err}"
        ))
    })?;

    if !metadata.is_file() {
        return Err(config_error(&format!(
            "SCORPIO_COPILOT_CLI_PATH points to '{path}', but it is not a file. Set SCORPIO_COPILOT_CLI_PATH to the Copilot CLI executable file."
        )));
    }

    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 == 0 {
        return Err(config_error(&format!(
            "SCORPIO_COPILOT_CLI_PATH points to '{path}', but it is not executable. Run 'chmod +x {path}' or point SCORPIO_COPILOT_CLI_PATH at the executable Copilot CLI binary."
        )));
    }

    Ok(())
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
    use std::{
        env,
        ffi::OsString,
        os::unix::fs::PermissionsExt,
        sync::{Mutex, OnceLock},
    };
    use tempfile::tempdir;

    fn copilot_cli_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_copilot_cli_env() -> std::sync::MutexGuard<'static, ()> {
        copilot_cli_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct CopilotCliPathEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        previous: Option<OsString>,
    }

    impl CopilotCliPathEnvGuard {
        fn set(value: &str) -> Self {
            let lock = lock_copilot_cli_env();
            let previous = env::var_os("SCORPIO_COPILOT_CLI_PATH");
            // SAFETY: Tests serialize access to this process-global variable via the mutex above.
            unsafe { env::set_var("SCORPIO_COPILOT_CLI_PATH", value) };
            Self {
                _lock: lock,
                previous,
            }
        }

        fn unset() -> Self {
            let lock = lock_copilot_cli_env();
            let previous = env::var_os("SCORPIO_COPILOT_CLI_PATH");
            // SAFETY: Tests serialize access to this process-global variable via the mutex above.
            unsafe { env::remove_var("SCORPIO_COPILOT_CLI_PATH") };
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for CopilotCliPathEnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => {
                    // SAFETY: Tests serialize access to this process-global variable via the mutex above.
                    unsafe { env::set_var("SCORPIO_COPILOT_CLI_PATH", previous) };
                }
                None => {
                    // SAFETY: Tests serialize access to this process-global variable via the mutex above.
                    unsafe { env::remove_var("SCORPIO_COPILOT_CLI_PATH") };
                }
            }
        }
    }

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
    fn factory_creates_copilot_client_without_api_key() {
        let dir = tempdir().expect("tempdir");
        let cli_path = dir.path().join("copilot");
        std::fs::write(&cli_path, "#!/bin/sh\nexit 0\n").expect("write cli stub");
        std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");
        let cli_path = cli_path.to_string_lossy().into_owned();
        let _guard = CopilotCliPathEnvGuard::set(&cli_path);
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "copilot");
        assert_eq!(handle.model_id(), "o3");
        assert!(matches!(handle.client, ProviderClient::Copilot(_)));
    }

    #[test]
    fn create_completion_model_attaches_provider_rate_limiter() {
        let cfg = sample_llm_config();
        let providers_cfg = ProvidersConfig {
            openai: crate::config::ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                base_url: None,
                rpm: 60,
            },
            ..ProvidersConfig::default()
        };
        let limiters = ProviderRateLimiters::from_config(&providers_cfg);

        let handle =
            create_completion_model(ModelTier::QuickThinking, &cfg, &providers_cfg, &limiters)
                .unwrap();

        assert_eq!(handle.rate_limiter().map(|l| l.label()), Some("openai"));
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

    // ── validate_copilot_cli_path ────────────────────────────────────────

    #[test]
    fn factory_copilot_requires_explicit_cli_path_env_var() {
        let _guard = CopilotCliPathEnvGuard::unset();
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let result = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no longer falls back to PATH lookup"),
            "expected missing Copilot CLI path error"
        );
    }

    #[test]
    fn factory_copilot_rejects_plain_cli_name_from_env() {
        let _guard = CopilotCliPathEnvGuard::set("copilot");
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let result = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("absolute path to an executable file"),
            "expected actionable absolute-path validation error"
        );
    }

    #[test]
    fn factory_copilot_rejects_nonexistent_cli_path_from_env() {
        let dir = tempdir().expect("tempdir");
        let missing_path = dir.path().join("missing-copilot");
        let missing_path = missing_path.to_string_lossy().into_owned();
        let _guard = CopilotCliPathEnvGuard::set(&missing_path);
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let result = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("does not exist"),
            "expected missing-file error in: {msg}"
        );
        assert!(
            msg.contains("Install the Copilot CLI and set SCORPIO_COPILOT_CLI_PATH"),
            "expected actionable setup guidance in: {msg}"
        );
    }

    #[test]
    fn factory_copilot_rejects_non_executable_cli_path_from_env() {
        let dir = tempdir().expect("tempdir");
        let cli_path = dir.path().join("copilot");
        std::fs::write(&cli_path, "#!/bin/sh\nexit 0\n").expect("write cli stub");
        std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o644))
            .expect("set non-executable permissions");
        let cli_path = cli_path.to_string_lossy().into_owned();
        let _guard = CopilotCliPathEnvGuard::set(&cli_path);
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let result = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("is not executable"),
            "expected non-executable error in: {msg}"
        );
        assert!(
            msg.contains("chmod +x"),
            "expected executable remediation hint in: {msg}"
        );
    }

    #[test]
    fn factory_copilot_accepts_executable_cli_path_from_env() {
        let dir = tempdir().expect("tempdir");
        let cli_path = dir.path().join("copilot");
        std::fs::write(&cli_path, "#!/bin/sh\nexit 0\n").expect("write cli stub");
        std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");
        let cli_path = cli_path.to_string_lossy().into_owned();
        let _guard = CopilotCliPathEnvGuard::set(&cli_path);
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );

        assert!(
            handle.is_ok(),
            "expected executable Copilot path to be accepted"
        );
    }

    #[tokio::test]
    async fn preflight_copilot_if_configured_ignores_non_copilot_providers() {
        let result = preflight_copilot_if_configured(
            &sample_llm_config(),
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        )
        .await;

        assert!(result.is_ok());
    }

    #[test]
    fn copilot_path_absolute_accepted() {
        let dir = tempdir().expect("tempdir");
        let cli_path = dir.path().join("copilot");
        std::fs::write(&cli_path, "#!/bin/sh\nexit 0\n").expect("write cli stub");
        std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o755))
            .expect("set executable permissions");

        assert!(validate_copilot_cli_path(cli_path.to_str().expect("utf8 path")).is_ok());
    }

    #[test]
    fn copilot_path_semicolon_rejected() {
        assert!(validate_copilot_cli_path("copilot;rm -rf /").is_err());
    }

    #[test]
    fn copilot_path_traversal_rejected() {
        assert!(validate_copilot_cli_path("../../bin/evil").is_err());
    }

    #[test]
    fn copilot_path_relative_with_slash_rejected() {
        assert!(validate_copilot_cli_path("bin/copilot").is_err());
    }

    #[test]
    fn copilot_path_plain_name_rejected() {
        assert!(validate_copilot_cli_path("copilot").is_err());
    }

    #[test]
    fn copilot_path_empty_after_trim_is_rejected() {
        assert!(resolve_copilot_exe_path_from("   ").is_err());
    }

    #[test]
    fn copilot_path_newline_rejected() {
        assert!(validate_copilot_cli_path("/usr/local/bin/copilot\n").is_err());
    }

    #[test]
    fn copilot_path_embedded_double_dot_segment_rejected() {
        assert!(validate_copilot_cli_path("/usr/local/bin/copilot..bak").is_err());
    }
}
