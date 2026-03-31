//! Unified LLM agent abstraction and builder functions.
//!
//! - [`LlmAgent`] — provider-agnostic agent with uniform `prompt`/`chat` interface.
//! - [`build_agent`] / [`build_agent_with_tools`] — construct agents from a
//!   [`CompletionModelHandle`] and a system prompt.
//! - [`prompt_typed`] — one-shot typed prompt dispatched over all provider variants.
//! - Mock infrastructure (`mock_llm_agent`, [`MockChatOutcome`], etc.) available under
//!   `#[cfg(test)]` for unit testing agent-using code without real LLM calls.

#[cfg(test)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[cfg(test)]
use rig::{OneOrMany, completion::AssistantContent, message::UserContent};
use rig::{
    agent::{PromptResponse, TypedPromptResponse},
    completion::{Chat, Message, Prompt, PromptError},
    tool::ToolDyn,
};
use serde::de::DeserializeOwned;

use crate::{
    error::TradingError,
    providers::{ProviderId, copilot::CopilotCompletionModel},
    rate_limit::SharedRateLimiter,
};

use super::client::{CompletionModelHandle, ProviderClient};
use super::error::map_structured_output_error_with_context;

// ────────────────────────────────────────────────────────────────────────────
// Type aliases for provider completion models
// ────────────────────────────────────────────────────────────────────────────

type OpenAIModel = rig::providers::openai::responses_api::ResponsesCompletionModel;
type AnthropicModel = rig::providers::anthropic::completion::CompletionModel;
type GeminiModel = rig::providers::gemini::completion::CompletionModel;
type OpenRouterModel = rig::providers::openrouter::completion::CompletionModel;

// ────────────────────────────────────────────────────────────────────────────
// LlmAgentInner (private dispatch enum)
// ────────────────────────────────────────────────────────────────────────────

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
    /// Agent backed by OpenRouter API aggregator.
    OpenRouter(rig::agent::Agent<OpenRouterModel>),
    #[cfg(test)]
    Mock(MockLlmAgent),
}

// ────────────────────────────────────────────────────────────────────────────
// Test mock infrastructure
// ────────────────────────────────────────────────────────────────────────────

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
            rate_limiter: None,
        },
        MockLlmAgentController {
            observed_history_lengths,
        },
    )
}

// ────────────────────────────────────────────────────────────────────────────
// LlmAgent public struct
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LlmAgent {
    provider: ProviderId,
    model_id: String,
    inner: LlmAgentInner,
    /// Rate limiter for this provider's LLM calls, or `None` if disabled.
    pub(crate) rate_limiter: Option<SharedRateLimiter>,
}

impl LlmAgent {
    pub fn provider_name(&self) -> &'static str {
        self.provider.as_str()
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Return the rate limiter for this agent's provider, if one is configured.
    pub fn rate_limiter(&self) -> Option<&SharedRateLimiter> {
        self.rate_limiter.as_ref()
    }

    /// Send a one-shot prompt and return the response text.
    pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
        match &self.inner {
            LlmAgentInner::OpenAI(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Anthropic(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Gemini(agent) => agent.prompt(prompt).await,
            LlmAgentInner::Copilot(agent) => agent.prompt(prompt).await,
            LlmAgentInner::OpenRouter(agent) => agent.prompt(prompt).await,
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
            LlmAgentInner::OpenRouter(agent) => agent.prompt(prompt).extended_details().await,
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
            LlmAgentInner::OpenRouter(agent) => agent
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
            LlmAgentInner::OpenRouter(agent) => agent.chat(prompt, chat_history).await,
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
            LlmAgentInner::OpenRouter(agent) => {
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

// ────────────────────────────────────────────────────────────────────────────
// MockLlmAgent impl (test only)
// ────────────────────────────────────────────────────────────────────────────

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

// ────────────────────────────────────────────────────────────────────────────
// Agent builder functions
// ────────────────────────────────────────────────────────────────────────────

/// Build a configured [`LlmAgent`] for the given tier with a system prompt.
///
/// This thin helper wraps `rig::AgentBuilder` so downstream agents don't repeat boilerplate.
/// Tools and structured output are **not** attached here — callers extend the agent
/// as needed after creation, or use [`build_agent_with_tools`] for tool-enabled agents.
///
/// # Errors
///
/// Returns `TradingError::Config` if the provider is unknown or the API key is missing
/// (delegated to [`super::client::create_completion_model`]).
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
                rate_limiter: handle.rate_limiter().cloned(),
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
        ProviderClient::OpenRouter(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(c, base, OpenRouter)
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Typed prompt helper
// ────────────────────────────────────────────────────────────────────────────

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
        LlmAgentInner::OpenRouter(a) => a.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        #[cfg(test)]
        LlmAgentInner::Mock(_) => Err(TradingError::Config(anyhow::anyhow!(
            "typed prompt not supported for mock llm agent"
        ))),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiConfig, LlmConfig};
    use crate::rate_limit::ProviderRateLimiters;
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

    fn api_config_with_openai() -> ApiConfig {
        ApiConfig {
            openai_api_key: Some(SecretString::from("test-key")),
            ..ApiConfig::default()
        }
    }

    fn api_config_with_anthropic() -> ApiConfig {
        ApiConfig {
            anthropic_api_key: Some(SecretString::from("test-key")),
            ..ApiConfig::default()
        }
    }

    fn api_config_with_gemini() -> ApiConfig {
        ApiConfig {
            gemini_api_key: Some(SecretString::from("test-key")),
            ..ApiConfig::default()
        }
    }

    fn api_config_with_openrouter() -> ApiConfig {
        ApiConfig {
            openrouter_api_key: Some(SecretString::from("test-openrouter-key")),
            ..ApiConfig::default()
        }
    }

    // ── Agent builder ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_agent_creates_openai_agent() {
        let cfg = sample_llm_config();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &api_config_with_openai(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "openai");
        assert_eq!(agent.model_id(), "gpt-4o-mini");
    }

    #[tokio::test]
    async fn build_agent_creates_anthropic_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &api_config_with_anthropic(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "anthropic");
        assert_eq!(agent.model_id(), "o3");
    }

    #[tokio::test]
    async fn build_agent_creates_gemini_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &api_config_with_gemini(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "gemini");
        assert_eq!(agent.model_id(), "o3");
    }

    #[tokio::test]
    async fn build_agent_creates_openrouter_agent() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3.6-plus-preview:free".to_owned();

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &api_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "openrouter");
        assert_eq!(agent.model_id(), "qwen/qwen3.6-plus-preview:free");
        assert!(matches!(&agent.inner, LlmAgentInner::OpenRouter(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_openrouter_deep_thinking_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "openrouter".to_owned();
        cfg.deep_thinking_model = "minimax/minimax-m2.5:free".to_owned();

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &api_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a deep-thinking test agent.");

        assert_eq!(agent.provider_name(), "openrouter");
        assert_eq!(agent.model_id(), "minimax/minimax-m2.5:free");
        assert!(matches!(&agent.inner, LlmAgentInner::OpenRouter(_)));
    }
}
