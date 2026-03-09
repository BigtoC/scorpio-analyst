//! Provider factory and unified agent abstraction.
//!
//! This module implements:
//! - [`ProviderClient`] enum dispatching over OpenAI, Anthropic, and Gemini rig clients.
//! - [`LlmAgent`] enum providing a uniform `prompt`/`chat` interface across providers.
//! - [`build_agent`] helper for constructing agents with a system prompt.
//! - [`prompt_with_retry`] and [`chat_with_retry`] wrappers applying timeout + exponential backoff.
//! - Error mapping from `rig` errors to [`TradingError`].

use std::time::Duration;

use rig::{
    completion::{Chat, Message, Prompt, PromptError, StructuredOutputError},
    providers::{anthropic, gemini, openai},
};
use secrecy::ExposeSecret;
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::{
    config::{ApiConfig, LlmConfig},
    error::{RetryPolicy, TradingError},
};

use super::ModelTier;

// ────────────────────────────────────────────────────────────────────────────
// Provider client enum
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
}

/// Construct a [`ProviderClient`] from configuration.
///
/// Resolves provider from the requested `tier`, then extracts the
/// corresponding API key from `api_config`. Returns `TradingError::Config` for unknown
/// providers or missing keys.
pub fn create_provider_client(
    tier: ModelTier,
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
) -> Result<ProviderClient, TradingError> {
    let provider = tier.provider_id(llm_config);
    match provider {
        "openai" => {
            let key = api_config
                .openai_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error("openai", "SCORPIO_OPENAI_API_KEY"))?;
            let client = openai::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create OpenAI client: {e}")))?;
            info!(provider = "openai", tier = %tier, "LLM provider client created");
            Ok(ProviderClient::OpenAI(client))
        }
        "anthropic" => {
            let key = api_config
                .anthropic_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error("anthropic", "SCORPIO_ANTHROPIC_API_KEY"))?;
            let client = anthropic::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create Anthropic client: {e}")))?;
            info!(provider = "anthropic", tier = %tier, "LLM provider client created");
            Ok(ProviderClient::Anthropic(client))
        }
        "gemini" => {
            let key = api_config
                .gemini_api_key
                .as_ref()
                .ok_or_else(|| missing_key_error("gemini", "SCORPIO_GEMINI_API_KEY"))?;
            let client = gemini::Client::new(key.expose_secret())
                .map_err(|e| config_error(&format!("failed to create Gemini client: {e}")))?;
            info!(provider = "gemini", tier = %tier, "LLM provider client created");
            Ok(ProviderClient::Gemini(client))
        }
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini)"
        ))),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Unified agent enum
// ────────────────────────────────────────────────────────────────────────────

/// Type aliases for each provider's concrete completion model (default HTTP client).
type OpenAIModel = rig::providers::openai::responses_api::ResponsesCompletionModel;
type AnthropicModel = rig::providers::anthropic::completion::CompletionModel;
type GeminiModel = rig::providers::gemini::completion::CompletionModel;

/// A provider-agnostic agent that implements uniform `prompt` and `chat` operations.
///
/// Each variant wraps a fully-configured `rig::agent::Agent<M>` for the corresponding
/// provider's completion model type.
#[derive(Clone)]
pub enum LlmAgent {
    /// Agent backed by OpenAI Responses API.
    OpenAI(rig::agent::Agent<OpenAIModel>),
    /// Agent backed by Anthropic Messages API.
    Anthropic(rig::agent::Agent<AnthropicModel>),
    /// Agent backed by Google Gemini API.
    Gemini(rig::agent::Agent<GeminiModel>),
}

impl LlmAgent {
    /// Send a one-shot prompt and return the response text.
    pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
        match self {
            Self::OpenAI(agent) => agent.prompt(prompt).await,
            Self::Anthropic(agent) => agent.prompt(prompt).await,
            Self::Gemini(agent) => agent.prompt(prompt).await,
        }
    }

    /// Send a prompt with chat history and return the response text.
    pub async fn chat(
        &self,
        prompt: &str,
        chat_history: Vec<Message>,
    ) -> Result<String, PromptError> {
        match self {
            Self::OpenAI(agent) => agent.chat(prompt, chat_history).await,
            Self::Anthropic(agent) => agent.chat(prompt, chat_history).await,
            Self::Gemini(agent) => agent.chat(prompt, chat_history).await,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Agent builder helper
// ────────────────────────────────────────────────────────────────────────────

/// Build a configured [`LlmAgent`] for the given tier with a system prompt.
///
/// This thin helper wraps `rig::AgentBuilder` so downstream agents don't repeat boilerplate.
/// Tools and structured output are **not** attached here — callers extend the agent
/// as needed after creation, or use [`build_agent_with_schema`] for typed output.
///
/// # Errors
///
/// Returns `TradingError::Config` if the provider is unknown or the API key is missing
/// (delegated to [`create_provider_client`]).
pub fn build_agent(
    client: &ProviderClient,
    llm_config: &LlmConfig,
    tier: ModelTier,
    system_prompt: &str,
) -> LlmAgent {
    let model_id = tier.model_id(llm_config);
    match client {
        ProviderClient::OpenAI(c) => {
            use rig::prelude::CompletionClient;
            let agent = c.agent(model_id).preamble(system_prompt).build();
            LlmAgent::OpenAI(agent)
        }
        ProviderClient::Anthropic(c) => {
            use rig::prelude::CompletionClient;
            let agent = c
                .agent(model_id)
                .preamble(system_prompt)
                .max_tokens(4096)
                .build();
            LlmAgent::Anthropic(agent)
        }
        ProviderClient::Gemini(c) => {
            use rig::prelude::CompletionClient;
            let agent = c.agent(model_id).preamble(system_prompt).build();
            LlmAgent::Gemini(agent)
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Error mapping
// ────────────────────────────────────────────────────────────────────────────

/// Map a `rig` [`PromptError`] to [`TradingError`].
///
/// Transport, provider, and tool errors become `TradingError::Rig`.
/// This preserves the original error message for diagnostics.
pub fn map_prompt_error(err: PromptError) -> TradingError {
    TradingError::Rig(err.to_string())
}

/// Map a `rig` [`StructuredOutputError`] to [`TradingError`].
///
/// Deserialization and empty-response failures become `TradingError::SchemaViolation`.
/// Underlying prompt/transport errors fall through to `TradingError::Rig`.
pub fn map_structured_output_error(err: StructuredOutputError) -> TradingError {
    match err {
        StructuredOutputError::DeserializationError(e) => TradingError::SchemaViolation {
            message: format!("failed to parse structured output: {e}"),
        },
        StructuredOutputError::EmptyResponse => TradingError::SchemaViolation {
            message: "model returned empty response for structured output".to_owned(),
        },
        StructuredOutputError::PromptError(e) => map_prompt_error(e),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Retry-wrapped completion helpers
// ────────────────────────────────────────────────────────────────────────────

/// Send a one-shot prompt with timeout and exponential-backoff retry.
///
/// Each attempt is guarded by `tokio::time::timeout(timeout)`. Transient errors
/// (rate limit, timeout) trigger a retry up to `policy.max_retries` times. Permanent
/// errors fail immediately.
///
/// # Errors
///
/// - `TradingError::Rig` for permanent provider/transport failures.
/// - `TradingError::NetworkTimeout` if all attempts exceed the timeout.
pub async fn prompt_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<String, TradingError> {
    let mut last_err = None;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            warn!(attempt, ?delay, "retrying prompt after transient error");
            tokio::time::sleep(delay).await;
        }

        match tokio::time::timeout(timeout, agent.prompt(prompt)).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, error = %err, "transient prompt error, will retry");
                    last_err = Some(map_prompt_error(err));
                    continue;
                }
                return Err(map_prompt_error(err));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: timeout,
                    message: format!("prompt timed out on attempt {attempt}"),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "prompt timed out, will retry");
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }
        }
    }

    // Should not reach here, but handle gracefully.
    Err(last_err.unwrap_or_else(|| TradingError::Rig("retry loop exhausted".to_owned())))
}

/// Send a chat prompt (with history) with timeout and exponential-backoff retry.
///
/// Behaves identically to [`prompt_with_retry`] but passes `chat_history` to the agent.
/// The history is cloned on each attempt so retries replay the full context.
///
/// # Errors
///
/// Same as [`prompt_with_retry`].
pub async fn chat_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &[Message],
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<String, TradingError> {
    let mut last_err = None;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            warn!(attempt, ?delay, "retrying chat after transient error");
            tokio::time::sleep(delay).await;
        }

        let history = chat_history.to_vec();
        match tokio::time::timeout(timeout, agent.chat(prompt, history)).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, error = %err, "transient chat error, will retry");
                    last_err = Some(map_prompt_error(err));
                    continue;
                }
                return Err(map_prompt_error(err));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: timeout,
                    message: format!("chat timed out on attempt {attempt}"),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "chat timed out, will retry");
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| TradingError::Rig("retry loop exhausted".to_owned())))
}

/// Prompt for a typed (structured) response, mapping schema failures to
/// `TradingError::SchemaViolation`.
///
/// This is a convenience for agents that need JSON-schema-constrained output.
/// It calls `prompt_typed` on the underlying rig agent, so the provider's native
/// structured output support is used when available.
///
/// Note: Retry logic is not applied here because `prompt_typed` goes through
/// a different code path (TypedPrompt). Callers should wrap this in their own
/// retry loop if needed.
pub async fn prompt_typed<T>(agent: &LlmAgent, prompt: &str) -> Result<T, TradingError>
where
    T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
{
    use rig::completion::TypedPrompt;
    match agent {
        LlmAgent::OpenAI(a) => a
            .prompt_typed::<T>(prompt)
            .await
            .map_err(map_structured_output_error),
        LlmAgent::Anthropic(a) => a
            .prompt_typed::<T>(prompt)
            .await
            .map_err(map_structured_output_error),
        LlmAgent::Gemini(a) => a
            .prompt_typed::<T>(prompt)
            .await
            .map_err(map_structured_output_error),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Classify whether a `PromptError` is likely transient (worth retrying).
///
/// Rate-limit and HTTP transport errors are considered transient.
/// Authentication, schema, and tool errors are permanent.
fn is_transient_error(err: &PromptError) -> bool {
    match err {
        PromptError::CompletionError(ce) => {
            let msg = ce.to_string().to_lowercase();
            // Rate-limit indicators from various providers
            msg.contains("rate limit")
                || msg.contains("429")
                || msg.contains("too many requests")
                // Transient transport / server errors
                || msg.contains("timeout")
                || msg.contains("connection")
                || msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
        }
        // Tool errors and cancellations are not transient
        PromptError::ToolError(_)
        | PromptError::ToolServerError(_)
        | PromptError::MaxTurnsError { .. }
        | PromptError::PromptCancelled { .. } => false,
    }
}

/// Convenience for creating `TradingError::Config` from a message.
fn config_error(msg: &str) -> TradingError {
    TradingError::Config(anyhow::anyhow!("{}", msg))
}

/// Convenience for creating a missing-API-key config error.
fn missing_key_error(provider: &str, env_var: &str) -> TradingError {
    config_error(&format!(
        "API key for provider \"{provider}\" is not set (expected env var: {env_var})"
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
            agent_timeout_secs: 30,
        }
    }

    fn empty_api_config() -> ApiConfig {
        ApiConfig {
            finnhub_rate_limit: 30,
            openai_api_key: None,
            anthropic_api_key: None,
            gemini_api_key: None,
            finnhub_api_key: None,
        }
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

    // ── Factory error paths ──────────────────────────────────────────────

    #[test]
    fn factory_unknown_provider_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "unsupported".to_owned();

        let result = create_provider_client(ModelTier::QuickThinking, &cfg, &empty_api_config());
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
        let result = create_provider_client(ModelTier::QuickThinking, &cfg, &empty_api_config());
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

        let result = create_provider_client(ModelTier::QuickThinking, &cfg, &empty_api_config());
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

        let result = create_provider_client(ModelTier::QuickThinking, &cfg, &empty_api_config());
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
        let client =
            create_provider_client(ModelTier::QuickThinking, &cfg, &api_config_with_openai());
        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::OpenAI(_)));
    }

    #[test]
    fn factory_creates_anthropic_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();

        let client =
            create_provider_client(ModelTier::DeepThinking, &cfg, &api_config_with_anthropic());
        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Anthropic(_)));
    }

    #[test]
    fn factory_creates_gemini_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();

        let client =
            create_provider_client(ModelTier::DeepThinking, &cfg, &api_config_with_gemini());
        assert!(client.is_ok());
        assert!(matches!(client.unwrap(), ProviderClient::Gemini(_)));
    }

    // ── Agent builder ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_agent_creates_openai_agent() {
        let cfg = sample_llm_config();
        let client =
            create_provider_client(ModelTier::QuickThinking, &cfg, &api_config_with_openai())
                .unwrap();
        let agent = build_agent(
            &client,
            &cfg,
            ModelTier::QuickThinking,
            "You are a test agent.",
        );
        assert!(matches!(agent, LlmAgent::OpenAI(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_anthropic_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();
        let client =
            create_provider_client(ModelTier::DeepThinking, &cfg, &api_config_with_anthropic())
                .unwrap();
        let agent = build_agent(
            &client,
            &cfg,
            ModelTier::DeepThinking,
            "You are a test agent.",
        );
        assert!(matches!(agent, LlmAgent::Anthropic(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_gemini_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();
        let client =
            create_provider_client(ModelTier::DeepThinking, &cfg, &api_config_with_gemini())
                .unwrap();
        let agent = build_agent(
            &client,
            &cfg,
            ModelTier::DeepThinking,
            "You are a test agent.",
        );
        assert!(matches!(agent, LlmAgent::Gemini(_)));
    }

    // ── Error mapping ────────────────────────────────────────────────────

    #[test]
    fn map_prompt_error_produces_rig_variant() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "test error".to_owned(),
        ));
        let mapped = map_prompt_error(err);
        assert!(matches!(mapped, TradingError::Rig(_)));
        assert!(mapped.to_string().contains("test error"));
    }

    #[test]
    fn map_structured_output_deserialization_error_produces_schema_violation() {
        let json_err = serde_json::from_str::<i32>("not a number").unwrap_err();
        let err = StructuredOutputError::DeserializationError(json_err);
        let mapped = map_structured_output_error(err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn map_structured_output_empty_response_produces_schema_violation() {
        let err = StructuredOutputError::EmptyResponse;
        let mapped = map_structured_output_error(err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
        assert!(mapped.to_string().contains("empty response"));
    }

    #[test]
    fn map_structured_output_prompt_error_falls_through_to_rig() {
        let inner = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "inner".to_owned(),
        ));
        let err = StructuredOutputError::PromptError(inner);
        let mapped = map_structured_output_error(err);
        assert!(matches!(mapped, TradingError::Rig(_)));
    }

    // ── Transient error classification ───────────────────────────────────

    #[test]
    fn rate_limit_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "rate limit exceeded".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn http_429_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "HTTP 429 Too Many Requests".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn server_500_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ResponseError(
            "Internal server error 500".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn auth_error_is_not_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "invalid API key".to_owned(),
        ));
        assert!(!is_transient_error(&err));
    }

    #[test]
    fn tool_error_is_not_transient() {
        use rig::tool::ToolSetError;
        let err = PromptError::ToolError(ToolSetError::ToolNotFoundError("foo".to_owned()));
        assert!(!is_transient_error(&err));
    }

    // ── Retry integration (timeout-based) ────────────────────────────────

    // Note: Full retry integration tests with mock completion models require either
    // a mock HTTP server or a custom `CompletionModel` impl. The following tests
    // validate the retry policy arithmetic and error classification, which are the
    // core logic tested without network calls.

    #[test]
    fn retry_policy_delay_arithmetic() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
        };
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(400));
    }
}
