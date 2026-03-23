//! Provider factory and unified agent abstraction.
//!
//! This module implements:
//! - [`ProviderClient`] enum dispatching over OpenAI, Anthropic, and Gemini rig clients.
//! - [`LlmAgent`] enum providing a uniform `prompt`/`chat` interface across providers.
//! - [`build_agent`] helper for constructing agents with a system prompt.
//! - [`prompt_with_retry`] and [`chat_with_retry`] wrappers applying timeout + exponential backoff.
//! - Error mapping from `rig` errors to [`TradingError`].

use std::time::{Duration, Instant};

#[cfg(test)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[cfg(test)]
use rig::{OneOrMany, completion::AssistantContent, message::UserContent};
use rig::{
    agent::{PromptResponse, TypedPromptResponse},
    completion::{Chat, Message, Prompt, PromptError, StructuredOutputError},
    providers::{anthropic, gemini, openai},
    tool::ToolDyn,
};
use secrecy::ExposeSecret;
use serde::de::DeserializeOwned;
use tracing::{info, warn};

use crate::{
    config::{ApiConfig, LlmConfig},
    error::{RetryPolicy, TradingError},
    providers::copilot::{CopilotCompletionModel, CopilotProviderClient},
};

use super::ModelTier;

const MAX_ERROR_SUMMARY_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    /// GitHub Copilot via ACP (no API key required; spawns local CLI).
    Copilot,
}

impl ProviderId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
        }
    }

    const fn missing_key_hint(self) -> &'static str {
        match self {
            Self::OpenAI => "SCORPIO_OPENAI_API_KEY",
            Self::Anthropic => "SCORPIO_ANTHROPIC_API_KEY",
            Self::Gemini => "SCORPIO_GEMINI_API_KEY",
            Self::Copilot => "(no API key required — install the Copilot CLI and authenticate)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompletionModelHandle {
    provider: ProviderId,
    model_id: String,
    client: ProviderClient,
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
        }
    }
}

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
    /// GitHub Copilot via ACP (local CLI subprocess, no API key).
    Copilot(CopilotProviderClient),
}

/// Construct a reusable completion-model handle from configuration.
///
/// Resolves provider from the requested `tier`, then extracts the
/// corresponding API key from `api_config`. Returns `TradingError::Config` for unknown
/// providers, invalid model IDs, or missing keys.
pub fn create_completion_model(
    tier: ModelTier,
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
) -> Result<CompletionModelHandle, TradingError> {
    let provider = validate_provider_id(tier.provider_id(llm_config))?;
    let model_id = validate_model_id(tier.model_id(llm_config))?;
    let client = create_provider_client_for(provider, api_config)?;
    info!(provider = provider.as_str(), model = model_id.as_str(), tier = %tier, "LLM completion model handle created");
    Ok(CompletionModelHandle {
        provider,
        model_id,
        client,
    })
}

/// Backwards-compatible helper that returns only the provider client.
pub fn create_provider_client(
    tier: ModelTier,
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
) -> Result<ProviderClient, TradingError> {
    create_completion_model(tier, llm_config, api_config).map(|handle| handle.client)
}

pub async fn preflight_configured_providers(
    llm_config: &LlmConfig,
    api_config: &ApiConfig,
) -> Result<(), TradingError> {
    for tier in [ModelTier::QuickThinking, ModelTier::DeepThinking] {
        let handle = create_completion_model(tier, llm_config, api_config)?;
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
            // Copilot requires no API key; the CLI executable path defaults to "copilot".
            // Use SCORPIO_COPILOT_CLI_PATH env var to override if needed.
            let exe_path =
                std::env::var("SCORPIO_COPILOT_CLI_PATH").unwrap_or_else(|_| "copilot".to_owned());
            validate_copilot_cli_path(&exe_path)?;
            Ok(ProviderClient::Copilot(CopilotProviderClient::new(
                exe_path,
            )))
        }
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
enum LlmAgentInner {
    /// Agent backed by OpenAI Responses API.
    OpenAI(rig::agent::Agent<OpenAIModel>),
    /// Agent backed by Anthropic Messages API.
    Anthropic(rig::agent::Agent<AnthropicModel>),
    /// Agent backed by Google Gemini API.
    Gemini(rig::agent::Agent<GeminiModel>),
    /// Agent backed by GitHub Copilot via ACP.
    Copilot(rig::agent::Agent<CopilotCompletionModel>),
    #[cfg(test)]
    Mock(MockLlmAgent),
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct MockLlmAgent {
    prompt_results: Arc<Mutex<VecDeque<Result<PromptResponse, PromptError>>>>,
    chat_results: Arc<Mutex<VecDeque<MockChatOutcome>>>,
    observed_prompts: Arc<Mutex<Vec<String>>>,
    observed_history_lengths: Arc<Mutex<Vec<usize>>>,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct MockLlmAgentController {
    observed_history_lengths: Arc<Mutex<Vec<usize>>>,
}

#[cfg(test)]
pub(crate) enum MockChatOutcome {
    Ok(PromptResponse),
    PartialUserThenErr(PromptError),
}

#[cfg(test)]
impl MockLlmAgentController {
    pub(crate) fn observed_history_lengths(&self) -> Vec<usize> {
        self.observed_history_lengths.lock().unwrap().clone()
    }
}

#[cfg(test)]
pub(crate) fn mock_prompt_response(output: &str, usage: rig::completion::Usage) -> PromptResponse {
    PromptResponse::new(output, usage)
}

#[cfg(test)]
pub(crate) fn mock_llm_agent(
    model_id: &str,
    prompt_results: Vec<Result<PromptResponse, PromptError>>,
    chat_results: Vec<MockChatOutcome>,
) -> (LlmAgent, MockLlmAgentController) {
    let observed_prompts = Arc::new(Mutex::new(Vec::new()));
    let observed_history_lengths = Arc::new(Mutex::new(Vec::new()));
    let inner = MockLlmAgent {
        prompt_results: Arc::new(Mutex::new(prompt_results.into())),
        chat_results: Arc::new(Mutex::new(chat_results.into())),
        observed_prompts: Arc::clone(&observed_prompts),
        observed_history_lengths: Arc::clone(&observed_history_lengths),
    };

    (
        LlmAgent {
            provider: ProviderId::OpenAI,
            model_id: model_id.to_owned(),
            inner: LlmAgentInner::Mock(inner),
        },
        MockLlmAgentController {
            observed_history_lengths,
        },
    )
}

impl LlmAgent {
    pub fn provider_name(&self) -> &'static str {
        self.provider.as_str()
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Send a one-shot prompt and return the response text.
    pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
        match &self.inner {
            LlmAgentInner::OpenAI(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Anthropic(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Gemini(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Copilot(agent) => agent.prompt(prompt).await,
            #[cfg(test)]
            LlmAgentInner::Mock(agent) => Ok(agent.prompt_details(prompt).await?.output),
        }
    }

    /// Send a one-shot prompt and return text plus aggregated usage details.
    pub async fn prompt_details(&self, prompt: &str) -> Result<PromptResponse, PromptError> {
        match &self.inner {
            LlmAgentInner::OpenAI(agent) => agent.prompt(prompt).extended_details().await,
            LlmAgentInner::Anthropic(agent) => agent.prompt(prompt).extended_details().await,
            LlmAgentInner::Gemini(agent) => agent.prompt(prompt).extended_details().await,
            LlmAgentInner::Copilot(agent) => agent.prompt(prompt).extended_details().await,
            #[cfg(test)]
            LlmAgentInner::Mock(agent) => agent.prompt_details(prompt).await,
        }
    }

    /// Send a typed prompt and return parsed output plus aggregated usage details.
    pub async fn prompt_typed_details<T>(
        &self,
        prompt: &str,
        max_turns: usize,
    ) -> Result<TypedPromptResponse<T>, TradingError>
    where
        T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
    {
        use rig::completion::TypedPrompt;

        // Capture the error-mapping closure once so each arm stays a single expression.
        let map_err = |err| {
            map_structured_output_error_with_context(self.provider_name(), self.model_id(), err)
        };

        match &self.inner {
            LlmAgentInner::OpenAI(agent) => agent
                .prompt_typed::<T>(prompt)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(map_err),
            LlmAgentInner::Anthropic(agent) => agent
                .prompt_typed::<T>(prompt)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(map_err),
            LlmAgentInner::Gemini(agent) => agent
                .prompt_typed::<T>(prompt)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(map_err),
            LlmAgentInner::Copilot(agent) => agent
                .prompt_typed::<T>(prompt)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(map_err),
            #[cfg(test)]
            LlmAgentInner::Mock(_) => Err(TradingError::Config(anyhow::anyhow!(
                "typed prompt not supported for mock llm agent"
            ))),
        }
    }

    /// Send a prompt with chat history and return the response text.
    pub async fn chat(
        &self,
        prompt: &str,
        chat_history: Vec<Message>,
    ) -> Result<String, PromptError> {
        match &self.inner {
            LlmAgentInner::OpenAI(agent) => agent.chat(prompt, chat_history).await,
            LlmAgentInner::Anthropic(agent) => agent.chat(prompt, chat_history).await,
            LlmAgentInner::Gemini(agent) => agent.chat(prompt, chat_history).await,
            LlmAgentInner::Copilot(agent) => agent.chat(prompt, chat_history).await,
            #[cfg(test)]
            LlmAgentInner::Mock(agent) => {
                let mut history = chat_history;
                Ok(agent.chat_details(prompt, &mut history).await?.output)
            }
        }
    }

    /// Send a prompt with mutable chat history and return response text plus usage details.
    ///
    /// The `chat_history` is updated in place: the new user message and the assistant
    /// response are appended so callers can pass the same `Vec<Message>` across rounds.
    pub async fn chat_details(
        &self,
        prompt: &str,
        chat_history: &mut Vec<Message>,
    ) -> Result<PromptResponse, PromptError> {
        use rig::agent::PromptRequest;

        match &self.inner {
            LlmAgentInner::OpenAI(agent) => {
                PromptRequest::from_agent(agent, prompt)
                    .with_history(chat_history)
                    .extended_details()
                    .await
            }
            LlmAgentInner::Anthropic(agent) => {
                PromptRequest::from_agent(agent, prompt)
                    .with_history(chat_history)
                    .extended_details()
                    .await
            }
            LlmAgentInner::Gemini(agent) => {
                PromptRequest::from_agent(agent, prompt)
                    .with_history(chat_history)
                    .extended_details()
                    .await
            }
            LlmAgentInner::Copilot(agent) => {
                PromptRequest::from_agent(agent, prompt)
                    .with_history(chat_history)
                    .extended_details()
                    .await
            }
            #[cfg(test)]
            LlmAgentInner::Mock(agent) => agent.chat_details(prompt, chat_history).await,
        }
    }
}

#[cfg(test)]
impl MockLlmAgent {
    async fn prompt_details(&self, prompt: &str) -> Result<PromptResponse, PromptError> {
        self.observed_prompts
            .lock()
            .unwrap()
            .push(prompt.to_owned());
        self.prompt_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(mock_prompt_response(
                    "",
                    rig::completion::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        total_tokens: 0,
                        cached_input_tokens: 0,
                    },
                ))
            })
    }

    async fn chat_details(
        &self,
        prompt: &str,
        chat_history: &mut Vec<Message>,
    ) -> Result<PromptResponse, PromptError> {
        self.observed_prompts
            .lock()
            .unwrap()
            .push(prompt.to_owned());
        self.observed_history_lengths
            .lock()
            .unwrap()
            .push(chat_history.len());

        let outcome = self
            .chat_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                MockChatOutcome::Ok(mock_prompt_response(
                    "",
                    rig::completion::Usage {
                        input_tokens: 0,
                        output_tokens: 0,
                        total_tokens: 0,
                        cached_input_tokens: 0,
                    },
                ))
            });

        chat_history.push(Message::User {
            content: OneOrMany::one(UserContent::text(prompt)),
        });

        match outcome {
            MockChatOutcome::Ok(response) => {
                chat_history.push(Message::Assistant {
                    content: OneOrMany::one(AssistantContent::text(response.output.clone())),
                    id: None,
                });
                Ok(response)
            }
            MockChatOutcome::PartialUserThenErr(err) => Err(err),
        }
    }
}

#[derive(Clone)]
pub struct LlmAgent {
    provider: ProviderId,
    model_id: String,
    inner: LlmAgentInner,
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
pub fn build_agent(handle: &CompletionModelHandle, system_prompt: &str) -> LlmAgent {
    build_agent_inner(handle, system_prompt, None)
}

/// Build a configured [`LlmAgent`] with a set of tools attached.
///
/// Tools are passed as `Vec<Box<dyn ToolDyn>>` to avoid type-parameter explosion —
/// rig's `AgentBuilder::tools()` accepts this and uses the `ToolServer` internally
/// to dispatch tool calls at runtime.
///
/// # Example
///
/// ```rust,ignore
/// let agent = build_agent_with_tools(
///     &handle,
///     "You are a financial analyst.",
///     vec![Box::new(StockPriceTool::new(client.clone()))],
/// );
/// ```
pub fn build_agent_with_tools(
    handle: &CompletionModelHandle,
    system_prompt: &str,
    tools: Vec<Box<dyn ToolDyn>>,
) -> LlmAgent {
    build_agent_inner(handle, system_prompt, Some(tools))
}

/// Shared builder core for [`build_agent`] and [`build_agent_with_tools`].
///
/// When `tools` is `None` the agent is constructed without tool bindings;
/// when `Some` the tools are attached via `AgentBuilder::tools`.
///
/// # Typestate note
///
/// `rig`'s `AgentBuilder` uses a typestate pattern: calling `.tools()` changes
/// the builder's type parameter from `NoToolConfig` to `WithBuilderTools`, making
/// it impossible to assign back to the same `let mut` binding. The macro therefore
/// has two branches — one for `None` (no tools) and one for `Some(t)` (with tools)
/// — rather than a conditional `builder = builder.tools(t)`.
fn build_agent_inner(
    handle: &CompletionModelHandle,
    system_prompt: &str,
    tools: Option<Vec<Box<dyn ToolDyn>>>,
) -> LlmAgent {
    // Produces the base builder (without Anthropic's extra `.max_tokens`) and
    // dispatches on `tools` to avoid the typestate assignment problem.
    macro_rules! make_agent {
        ($client:expr, $base_builder:expr, $variant:ident) => {{
            let agent = match tools {
                None => $base_builder.build(),
                Some(t) => $base_builder.tools(t).build(),
            };
            LlmAgent {
                provider: handle.provider_id(),
                model_id: handle.model_id().to_owned(),
                inner: LlmAgentInner::$variant(agent),
            }
        }};
    }

    match &handle.client {
        ProviderClient::OpenAI(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(c, base, OpenAI)
        }
        ProviderClient::Anthropic(c) => {
            use rig::prelude::CompletionClient;
            let base = c
                .agent(handle.model_id())
                .preamble(system_prompt)
                .max_tokens(4096);
            make_agent!(c, base, Anthropic)
        }
        ProviderClient::Gemini(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(c, base, Gemini)
        }
        ProviderClient::Copilot(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(c, base, Copilot)
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Error mapping
// ────────────────────────────────────────────────────────────────────────────

/// Map a `rig` [`PromptError`] to [`TradingError`].
///
/// Transport, provider, and tool errors become `TradingError::Rig` with sanitized context.
pub fn map_prompt_error(err: PromptError) -> TradingError {
    map_prompt_error_with_context("unknown", "unknown", err)
}

fn map_prompt_error_with_context(provider: &str, model_id: &str, err: PromptError) -> TradingError {
    TradingError::Rig(format!(
        "provider={provider} model={model_id} summary={}",
        sanitize_error_summary(&err.to_string())
    ))
}

/// Map a `rig` [`StructuredOutputError`] to [`TradingError`].
///
/// Deserialization and empty-response failures become `TradingError::SchemaViolation`.
/// Underlying prompt/transport errors fall through to `TradingError::Rig`.
pub fn map_structured_output_error(err: StructuredOutputError) -> TradingError {
    map_structured_output_error_with_context("unknown", "unknown", err)
}

fn map_structured_output_error_with_context(
    provider: &str,
    model_id: &str,
    err: StructuredOutputError,
) -> TradingError {
    match err {
        StructuredOutputError::DeserializationError(_e) => {
            // Do not surface the raw serde error — it can contain a fragment of the
            // LLM's response text, which may include sensitive content.
            tracing::debug!(
                provider,
                model_id,
                error = %_e,
                "structured output deserialization failed"
            );
            TradingError::SchemaViolation {
                message: format!(
                    "provider={provider} model={model_id}: structured output could not be parsed"
                ),
            }
        }
        StructuredOutputError::EmptyResponse => TradingError::SchemaViolation {
            message: format!("provider={provider} model={model_id}: model returned empty response"),
        },
        StructuredOutputError::PromptError(e) => {
            map_prompt_error_with_context(provider, model_id, e)
        }
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
    let total_budget = total_request_budget(timeout, policy);
    prompt_with_retry_budget(agent, prompt, timeout, total_budget, policy).await
}

/// Send a one-shot prompt with timeout/retry and return extended details including usage.
pub async fn prompt_with_retry_details(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<PromptResponse, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    prompt_with_retry_details_budget(agent, prompt, timeout, total_budget, policy).await
}

pub(crate) async fn prompt_with_retry_details_budget(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<PromptResponse, TradingError> {
    retry_prompt_budget_loop(agent, timeout, total_budget, policy, || {
        agent.prompt_details(prompt)
    })
    .await
}

pub(crate) async fn prompt_with_retry_budget(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<String, TradingError> {
    retry_prompt_budget_loop(agent, timeout, total_budget, policy, || {
        agent.prompt(prompt)
    })
    .await
}

/// Shared retry-loop core for [`prompt_with_retry_budget`] and
/// [`prompt_with_retry_details_budget`].
///
/// `call_fn` is invoked on each attempt and must return a `Future` that resolves to
/// `Result<R, PromptError>`. The two callers differ only in which `LlmAgent` method
/// they invoke (`prompt` vs `prompt_details`).
async fn retry_prompt_budget_loop<R, F, Fut>(
    agent: &LlmAgent,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
    call_fn: F,
) -> Result<R, TradingError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<R, PromptError>>,
{
    let started_at = Instant::now();

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "prompt retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(attempt, ?delay, "retrying prompt after transient error");
            tokio::time::sleep(delay).await;
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "prompt retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, call_fn()).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient prompt error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "prompt timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "prompt timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
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
    let total_budget = total_request_budget(timeout, policy);
    chat_with_retry_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

pub async fn chat_with_retry_budget(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &[Message],
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<String, TradingError> {
    let started_at = Instant::now();

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(attempt, ?delay, "retrying chat after transient error");
            tokio::time::sleep(delay).await;
        }

        let history = chat_history.to_vec();
        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "chat retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, agent.chat(prompt, history)).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient chat error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "chat timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "chat timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

/// Send a chat prompt (with mutable history) with timeout/retry and return response plus usage.
///
/// The `chat_history` is updated in place by appending each new message pair. This is the
/// correct API for multi-turn debates where callers maintain history across rounds.
///
/// # Errors
///
/// Same as [`chat_with_retry`].
pub async fn chat_with_retry_details(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &mut Vec<Message>,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<PromptResponse, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    chat_with_retry_details_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

/// Budget-constrained variant of [`chat_with_retry_details`].
pub async fn chat_with_retry_details_budget(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &mut Vec<Message>,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<PromptResponse, TradingError> {
    let started_at = Instant::now();

    // Snapshot the history length before each attempt so we can truncate on retry.
    let initial_len = chat_history.len();

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat-details retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(
                attempt,
                ?delay,
                "retrying chat-details after transient error"
            );
            // Truncate any partial messages that were appended during the failed attempt.
            chat_history.truncate(initial_len);
            tokio::time::sleep(delay).await;
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "chat-details retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, agent.chat_details(prompt, chat_history)).await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient chat-details error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                // On timeout, also truncate any partial messages.
                chat_history.truncate(initial_len);
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "chat-details timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "chat-details timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
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
    match &agent.inner {
        LlmAgentInner::OpenAI(a) => a.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        LlmAgentInner::Anthropic(a) => a.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        LlmAgentInner::Gemini(a) => a.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        LlmAgentInner::Copilot(a) => a.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        #[cfg(test)]
        LlmAgentInner::Mock(_) => Err(TradingError::Config(anyhow::anyhow!(
            "typed prompt not supported for mock llm agent"
        ))),
    }
}

/// Prompt for a typed response and return usage metadata from the underlying agent loop.
pub async fn prompt_typed_with_retry<T>(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    max_turns: usize,
) -> Result<TypedPromptResponse<T>, TradingError>
where
    T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
{
    let total_budget = total_request_budget(timeout, policy);
    let started_at = Instant::now();

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "typed prompt retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(
                attempt,
                ?delay,
                "retrying typed prompt after transient error"
            );
            tokio::time::sleep(delay).await;
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "typed prompt retry budget exhausted".to_owned(),
            });
        }

        let attempt_timeout = timeout.min(remaining_budget);
        match tokio::time::timeout(
            attempt_timeout,
            agent.prompt_typed_details::<T>(prompt, max_turns),
        )
        .await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(err)) => {
                if should_retry_typed_error(&err) && attempt < policy.max_retries {
                    continue;
                }
                return Err(err);
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "typed prompt timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
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

fn should_retry_typed_error(err: &TradingError) -> bool {
    match err {
        TradingError::NetworkTimeout { .. } | TradingError::RateLimitExceeded { .. } => true,
        // SchemaViolation is a permanent failure for a given LLM output — the same
        // prompt to the same model is unlikely to produce a valid response on retry,
        // and retrying wastes token budget. Fail fast on schema errors.
        TradingError::SchemaViolation { .. } => false,
        TradingError::Rig(message) => {
            let msg = message.to_ascii_lowercase();
            msg.contains("rate limit")
                || msg.contains("429")
                || msg.contains("too many requests")
                || msg.contains("timeout")
                || msg.contains("connection")
                || msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
        }
        TradingError::AnalystError { .. } | TradingError::Config(_) | TradingError::Storage(_) => {
            false
        }
        // GraphFlow errors originate from the orchestration layer, not from LLM providers,
        // so retrying the typed prompt won't help.
        TradingError::GraphFlow { .. } => false,
    }
}

fn validate_provider_id(provider: &str) -> Result<ProviderId, TradingError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(ProviderId::OpenAI),
        "anthropic" => Ok(ProviderId::Anthropic),
        "gemini" => Ok(ProviderId::Gemini),
        "copilot" => Ok(ProviderId::Copilot),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, copilot)"
        ))),
    }
}

/// Validate the Copilot CLI executable path supplied via `SCORPIO_COPILOT_CLI_PATH`.
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
    Ok(trimmed.to_owned())
}

/// Replace ASCII/Unicode control characters (except `\n` and `\t`) with a space.
pub(crate) fn replace_control_chars(s: &str) -> String {
    s.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// Redact known credential patterns (API key prefixes, auth headers, bearer tokens).
pub(crate) fn redact_credentials(s: &str) -> String {
    fn mask_prefixed_token(input: &str, prefix: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let prefix_bytes = prefix.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(prefix_bytes) {
                out.push_str("[REDACTED]");
                i += prefix_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                        i += ch.len_utf8();
                    } else {
                        break;
                    }
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    fn mask_assignment(input: &str, key: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let key_bytes = key.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(key_bytes) {
                out.push_str("[REDACTED]");
                i += key_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch.is_whitespace() || matches!(ch, '&' | ',' | ';' | ')' | ']' | '}') {
                        break;
                    }
                    i += ch.len_utf8();
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    fn mask_bearer(input: &str, prefix: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let bytes = input.as_bytes();
        let prefix_bytes = prefix.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i..].starts_with(prefix_bytes) {
                out.push_str("[REDACTED]");
                i += prefix_bytes.len();
                while i < bytes.len() {
                    let ch = input[i..].chars().next().unwrap();
                    if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
                        break;
                    }
                    i += ch.len_utf8();
                }
            } else {
                let ch = input[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
            }
        }

        out
    }

    let mut out = s.to_owned();
    for prefix in ["sk-ant-", "sk-", "AIza", "aiza"] {
        out = mask_prefixed_token(&out, prefix);
    }
    for key in ["api_key=", "api-key=", "apikey=", "token="] {
        out = mask_assignment(&out, key);
    }
    for prefix in ["Bearer ", "bearer ", "BEARER "] {
        out = mask_bearer(&out, prefix);
    }
    out = out.replace("Authorization:", "[REDACTED]");
    out = out.replace("authorization:", "[REDACTED]");
    out = out.replace("AUTHORIZATION:", "[REDACTED]");
    out
}

/// Truncate `s` to at most `max_chars` Unicode scalar values, appending `"..."` if trimmed.
pub(crate) fn truncate_to(s: &str, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        format!("{truncated}...")
    } else {
        truncated
    }
}

pub(crate) fn sanitize_error_summary(input: &str) -> String {
    let sanitized = replace_control_chars(input);
    let sanitized = redact_credentials(&sanitized);
    truncate_to(&sanitized, MAX_ERROR_SUMMARY_CHARS)
}

fn total_request_budget(timeout: Duration, policy: &RetryPolicy) -> Duration {
    let attempts = policy.max_retries.saturating_add(1);
    let base_budget = timeout.saturating_mul(attempts);
    let backoff_budget = (0..policy.max_retries).fold(Duration::ZERO, |acc, attempt| {
        acc.saturating_add(policy.delay_for_attempt(attempt))
    });
    base_budget.saturating_add(backoff_budget)
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

    fn api_config_for_copilot() -> ApiConfig {
        empty_api_config()
    }

    // ── Factory error paths ──────────────────────────────────────────────

    #[test]
    fn factory_unknown_provider_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "unsupported".to_owned();

        let result = create_completion_model(ModelTier::QuickThinking, &cfg, &empty_api_config());
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
        let result = create_completion_model(ModelTier::QuickThinking, &cfg, &empty_api_config());
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

        let result = create_completion_model(ModelTier::QuickThinking, &cfg, &empty_api_config());
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

        let result = create_completion_model(ModelTier::QuickThinking, &cfg, &empty_api_config());
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
        let handle =
            create_completion_model(ModelTier::QuickThinking, &cfg, &api_config_with_openai());
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "openai");
        assert_eq!(handle.model_id(), "gpt-4o-mini");
    }

    #[test]
    fn factory_creates_anthropic_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();

        let handle =
            create_completion_model(ModelTier::DeepThinking, &cfg, &api_config_with_anthropic());
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "anthropic");
        assert_eq!(handle.model_id(), "o3");
    }

    #[test]
    fn factory_creates_gemini_client() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();

        let handle =
            create_completion_model(ModelTier::DeepThinking, &cfg, &api_config_with_gemini());
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "gemini");
        assert_eq!(handle.model_id(), "o3");
    }

    #[test]
    fn factory_empty_model_id_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_model = "   ".to_owned();

        let result =
            create_completion_model(ModelTier::QuickThinking, &cfg, &api_config_with_openai());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("model ID"));
    }

    #[test]
    fn factory_creates_copilot_client_without_api_key() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "copilot".to_owned();

        let handle =
            create_completion_model(ModelTier::DeepThinking, &cfg, &api_config_for_copilot());
        assert!(handle.is_ok());
        let handle = handle.unwrap();
        assert_eq!(handle.provider_name(), "copilot");
        assert_eq!(handle.model_id(), "o3");
    }

    // ── Agent builder ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_agent_creates_openai_agent() {
        let cfg = sample_llm_config();
        let handle =
            create_completion_model(ModelTier::QuickThinking, &cfg, &api_config_with_openai())
                .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "openai");
        assert_eq!(agent.model_id(), "gpt-4o-mini");
    }

    #[tokio::test]
    async fn build_agent_creates_anthropic_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();
        let handle =
            create_completion_model(ModelTier::DeepThinking, &cfg, &api_config_with_anthropic())
                .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "anthropic");
        assert_eq!(agent.model_id(), "o3");
    }

    #[tokio::test]
    async fn build_agent_creates_gemini_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();
        let handle =
            create_completion_model(ModelTier::DeepThinking, &cfg, &api_config_with_gemini())
                .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "gemini");
        assert_eq!(agent.model_id(), "o3");
    }

    // ── Error mapping ────────────────────────────────────────────────────

    #[test]
    fn map_prompt_error_produces_rig_variant() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "test error".to_owned(),
        ));
        let mapped = map_prompt_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::Rig(_)));
        assert!(mapped.to_string().contains("openai"));
        assert!(mapped.to_string().contains("gpt-4o-mini"));
    }

    #[test]
    fn map_structured_output_deserialization_error_produces_schema_violation() {
        let json_err = serde_json::from_str::<i32>("not a number").unwrap_err();
        let err = StructuredOutputError::DeserializationError(json_err);
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
    }

    #[test]
    fn map_structured_output_empty_response_produces_schema_violation() {
        let err = StructuredOutputError::EmptyResponse;
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::SchemaViolation { .. }));
        assert!(mapped.to_string().contains("empty response"));
    }

    #[test]
    fn map_structured_output_prompt_error_falls_through_to_rig() {
        let inner = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "inner".to_owned(),
        ));
        let err = StructuredOutputError::PromptError(inner);
        let mapped = map_structured_output_error_with_context("openai", "gpt-4o-mini", err);
        assert!(matches!(mapped, TradingError::Rig(_)));
    }

    #[test]
    fn sanitize_error_summary_redacts_secret_like_values() {
        let sanitized = sanitize_error_summary("authorization failed for sk-secret-value");
        assert!(!sanitized.contains("sk-secret-value"));
        assert!(sanitized.contains("[REDACTED]"));
    }

    #[test]
    fn total_request_budget_includes_retry_delays() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(100),
        };
        let budget = total_request_budget(Duration::from_secs(1), &policy);
        assert_eq!(budget, Duration::from_millis(3300));
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

    // ── sanitize_error_summary (expanded) ────────────────────────────────

    #[test]
    fn redacts_gemini_api_key_prefix() {
        let result = sanitize_error_summary("key=AIzaSyTest1234");
        assert!(
            !result.contains("AIza"),
            "Gemini key prefix must be redacted"
        );
        assert!(
            !result.contains("SyTest1234"),
            "Gemini key body must be redacted"
        );
    }

    #[test]
    fn redacts_bearer_token() {
        let result = sanitize_error_summary("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9");
        assert!(!result.contains("Bearer "), "Bearer token must be redacted");
        assert!(
            !result.contains("eyJhbGciOiJIUzI1NiJ9"),
            "Bearer token body must be redacted"
        );
    }

    #[test]
    fn redacts_api_key_eq() {
        let result = sanitize_error_summary("request failed: api_key=secret123");
        assert!(!result.contains("api_key="), "api_key= must be redacted");
        assert!(
            !result.contains("secret123"),
            "api_key value must be redacted"
        );
    }

    #[test]
    fn redacts_openai_style_key_body() {
        let result = sanitize_error_summary("provider said sk-live-abc123XYZ failed");
        assert!(!result.contains("sk-live-abc123XYZ"));
        assert!(!result.contains("abc123XYZ"));
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

    // ── schema_violation_not_retried ─────────────────────────────────────

    #[test]
    fn schema_violation_is_not_retryable() {
        let err = TradingError::SchemaViolation {
            message: "bad output".to_owned(),
        };
        assert!(
            !should_retry_typed_error(&err),
            "SchemaViolation must not be retried"
        );
    }

    #[test]
    fn network_timeout_is_retryable() {
        let err = TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "timed out".to_owned(),
        };
        assert!(should_retry_typed_error(&err));
    }

    #[tokio::test]
    async fn chat_with_retry_details_retries_and_truncates_partial_history() {
        let (agent, controller) = mock_llm_agent(
            "o3",
            vec![],
            vec![
                MockChatOutcome::PartialUserThenErr(PromptError::CompletionError(
                    rig::completion::CompletionError::ResponseError("rate limit 429".to_owned()),
                )),
                MockChatOutcome::Ok(mock_prompt_response(
                    "Recovered response",
                    rig::completion::Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                        cached_input_tokens: 0,
                    },
                )),
            ],
        );

        let mut history = vec![Message::User {
            content: OneOrMany::one(UserContent::text("initial context")),
        }];

        let response = chat_with_retry_details_budget(
            &agent,
            "next prompt",
            &mut history,
            Duration::from_millis(50),
            Duration::from_millis(200),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.output, "Recovered response");
        assert_eq!(response.usage.total_tokens, 15);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(history.len(), 3);
        assert_eq!(controller.observed_history_lengths(), vec![1, 1]);
    }
}
