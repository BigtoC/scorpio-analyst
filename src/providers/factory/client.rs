//! Provider client construction and configuration validation.
//!
//! - [`ProviderClient`] — enum dispatching over concrete `rig` provider clients.
//! - [`CompletionModelHandle`] — reusable handle bundling provider, model ID, client, and rate limiter.
//! - [`create_completion_model`] — construct a handle from tier + config.
//! - [`preflight_configured_providers`] — validate provider connectivity before the pipeline starts.

use rig::providers::{anthropic, gemini, openai, openrouter};
use secrecy::ExposeSecret;
use tracing::info;

use crate::{
    config::{ApiConfig, LlmConfig},
    error::TradingError,
    providers::{
        ModelTier, ProviderId,
        copilot::CopilotProviderClient,
    },
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
pub enum ProviderClient {
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
/// The `rate_limiters` registry is used to attach a per-provider rate limiter to the
/// handle. Pass `&ProviderRateLimiters::default()` to disable rate limiting.
pub fn create_completion_model(
    tier: ModelTier,
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
    rate_limiters: &ProviderRateLimiters,
) -> Result<CompletionModelHandle, TradingError> {
    let provider = validate_provider_id(tier.provider_id(llm_config))?;
    let model_id = validate_model_id(tier.model_id(llm_config))?;
    let client = create_provider_client_for(provider, api_config, &model_id)?;
    let rate_limiter = rate_limiters.get(provider).cloned();
    info!(provider = provider.as_str(), model = model_id.as_str(), tier = %tier, "LLM completion model handle created");
    Ok(CompletionModelHandle {
        provider,
        model_id,
        client,
        rate_limiter,
    })
}

/// Backwards-compatible helper that returns only the provider client.
pub fn create_provider_client(
    tier: ModelTier,
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
) -> Result<ProviderClient, TradingError> {
    create_completion_model(
        tier,
        llm_config,
        api_config,
        &ProviderRateLimiters::default(),
    )
    .map(|handle| handle.client)
}

pub async fn preflight_configured_providers(
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
    rate_limiters: &ProviderRateLimiters,
) -> Result<(), TradingError> {
    for tier in [ModelTier::QuickThinking, ModelTier::DeepThinking] {
        let handle = create_completion_model(tier, llm_config, api_config, rate_limiters)?;
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
    api_config: &ApiConfig,
    model_id: &str,
) -> Result<ProviderClient, TradingError> {
    match provider {
        ProviderId::OpenAI => {
            let key = api_config
                .openai_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = openai::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create OpenAI client: {e}")))?;
            Ok(ProviderClient::OpenAI(client))
        }
        ProviderId::Anthropic => {
            let key = api_config
                .anthropic_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = anthropic::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create Anthropic client: {e}")))?;
            Ok(ProviderClient::Anthropic(client))
        }
        ProviderId::Gemini => {
            let key = api_config
                .gemini_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = gemini::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create Gemini client: {e}")))?;
            Ok(ProviderClient::Gemini(client))
        }
        ProviderId::Copilot => {
            // Copilot requires no API key. Resolve the CLI path in priority order:
            // 1. SCORPIO_COPILOT_CLI_PATH env var (explicit override)
            // 2. `which copilot` (absolute path from PATH)
            // 3. "copilot" plain name (last resort, relies on PATH at exec time)
            let exe_path = std::env::var("SCORPIO_COPILOT_CLI_PATH")
                .unwrap_or_else(|_| resolve_copilot_exe_path());
            validate_copilot_cli_path(&exe_path)?;
            Ok(ProviderClient::Copilot(CopilotProviderClient::new(
                exe_path, model_id,
            )))
        }
        ProviderId::OpenRouter => {
            let key = api_config
                .openrouter_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = openrouter::Client::new(key.expose_secret())
                .map_err(|e| {
                    config_error(&format!("failed to create OpenRouter client: {e}"))
                })?;
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

/// Resolve the absolute path to the `copilot` CLI using `which`.
///
/// Returns the trimmed stdout of `which copilot` on success, or falls back to
/// the plain name `"copilot"` if `which` is unavailable or returns no output.
fn resolve_copilot_exe_path() -> String {
    std::process::Command::new("which")
        .arg("copilot")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "copilot".to_owned())
}

/// Validate the Copilot CLI executable path.
///
/// Rejects paths that:
/// - Contain shell metacharacters that could enable injection.
/// - Contain `..` path-traversal sequences.
/// - Are relative paths containing `/` but not starting with `/` (must be either
///   a plain filename or an absolute path).
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
    // Relative paths with '/' (but not absolute) are ambiguous and disallowed.
    if path.contains('/') && !path.starts_with('/') {
        return Err(config_error(
            "SCORPIO_COPILOT_CLI_PATH must be a plain executable name or an absolute path",
        ));
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
    use crate::config::{ApiConfig, LlmConfig};
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
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    fn empty_api_config() -> ApiConfig {
        ApiConfig::default()
    }

    fn api_config_with_openai() -> ApiConfig {
        ApiConfig {
            openai_api_key: Some(SecretString::from("test-key")),
            ..empty_api_config()
        }
    }

    fn api_config_with_anthropic() -> ApiConfig {
        ApiConfig {
            anthropic_api_key: Some(SecretString::from("test-key")),
            ..empty_api_config()
        }
    }

    fn api_config_with_gemini() -> ApiConfig {
        ApiConfig {
            gemini_api_key: Some(SecretString::from("test-key")),
            ..empty_api_config()
        }
    }

    fn api_config_for_copilot() -> ApiConfig {
        empty_api_config()
    }

    fn api_config_with_openrouter() -> ApiConfig {
        ApiConfig {
            openrouter_api_key: Some(SecretString::from("test-openrouter-key")),
            ..empty_api_config()
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
            &empty_api_config(),
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
            &empty_api_config(),
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
            &empty_api_config(),
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
            &empty_api_config(),
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
            &api_config_with_openai(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "openai");
        assert_eq!(handle.model_id(), "gpt-4o-mini");
    }

    #[test]
    fn factory_creates_anthropic_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &api_config_with_anthropic(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "anthropic");
        assert_eq!(handle.model_id(), "o3");
    }

    #[test]
    fn factory_creates_gemini_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &api_config_with_gemini(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "gemini");
        assert_eq!(handle.model_id(), "o3");
    }

    #[test]
    fn factory_empty_model_id_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_model = "   ".to_owned();

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &api_config_with_openai(),
            &ProviderRateLimiters::default(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("model ID"));
    }

    #[test]
    fn factory_creates_copilot_client_without_api_key() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let handle = create_completion_model(
            ModelTier::DeepThinking,
            &cfg,
            &api_config_for_copilot(),
            &ProviderRateLimiters::default(),
        );
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "copilot");
        assert_eq!(handle.model_id(), "o3");
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

        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &empty_api_config(),
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
            &api_config_with_openrouter(),
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
            &api_config_with_openrouter(),
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
                &api_config_with_openrouter(),
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
            &api_config_with_openrouter(),
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
    fn copilot_path_plain_name_accepted() {
        assert!(validate_copilot_cli_path("copilot").is_ok());
    }

    #[test]
    fn copilot_path_absolute_accepted() {
        assert!(validate_copilot_cli_path("/usr/local/bin/copilot").is_ok());
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
}
