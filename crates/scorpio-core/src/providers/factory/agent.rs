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
    time::Duration,
};

#[cfg(test)]
type TypedResultQueue = Arc<Mutex<VecDeque<Result<Box<dyn std::any::Any + Send>, TradingError>>>>;

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
type DeepSeekModel = rig::providers::deepseek::CompletionModel;

macro_rules! dispatch_llm_agent {
    ($inner:expr, |$agent:ident| $body:expr, mock = |$mock:ident| $mock_body:expr) => {
        match $inner {
            LlmAgentInner::OpenAI($agent) => $body,
            LlmAgentInner::Anthropic($agent) => $body,
            LlmAgentInner::Gemini($agent) => $body,
            LlmAgentInner::Copilot($agent) => $body,
            LlmAgentInner::OpenRouter($agent) => $body,
            LlmAgentInner::DeepSeek($agent) => $body,
            #[cfg(test)]
            LlmAgentInner::Mock($mock) => $mock_body,
        }
    };
}

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
    /// Agent backed by DeepSeek API.
    DeepSeek(rig::agent::Agent<DeepSeekModel>),
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
    typed_results: TypedResultQueue,
    text_turn_results: Arc<Mutex<VecDeque<Result<PromptResponse, TradingError>>>>,
    observed_prompts: Arc<Mutex<Vec<String>>>,
    observed_history_lengths: Arc<Mutex<Vec<usize>>>,
    observed_history_ptrs: Arc<Mutex<Vec<usize>>>,
    observed_max_turns: Arc<Mutex<Vec<usize>>>,
    prompt_delay: Arc<Mutex<Duration>>,
    text_turn_delay: Arc<Mutex<Duration>>,
    typed_attempts: Arc<Mutex<usize>>,
    prompt_attempts: Arc<Mutex<usize>>,
    text_turn_attempts: Arc<Mutex<usize>>,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct MockLlmAgentController {
    observed_history_lengths: Arc<Mutex<Vec<usize>>>,
    observed_history_ptrs: Arc<Mutex<Vec<usize>>>,
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

    pub(crate) fn observed_history_ptrs(&self) -> Vec<usize> {
        self.observed_history_ptrs.lock().unwrap().clone()
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
    mock_llm_agent_with_provider_id(ProviderId::OpenAI, model_id, prompt_results, chat_results)
}

#[cfg(test)]
pub(crate) fn mock_llm_agent_with_provider_id(
    provider: ProviderId,
    model_id: &str,
    prompt_results: Vec<Result<PromptResponse, PromptError>>,
    chat_results: Vec<MockChatOutcome>,
) -> (LlmAgent, MockLlmAgentController) {
    let observed_prompts = Arc::new(Mutex::new(Vec::new()));
    let observed_history_lengths = Arc::new(Mutex::new(Vec::new()));
    let observed_history_ptrs = Arc::new(Mutex::new(Vec::new()));
    let inner = MockLlmAgent {
        prompt_results: Arc::new(Mutex::new(prompt_results.into())),
        chat_results: Arc::new(Mutex::new(chat_results.into())),
        typed_results: Arc::new(Mutex::new(VecDeque::new())),
        text_turn_results: Arc::new(Mutex::new(VecDeque::new())),
        observed_prompts: Arc::clone(&observed_prompts),
        observed_history_lengths: Arc::clone(&observed_history_lengths),
        observed_history_ptrs: Arc::clone(&observed_history_ptrs),
        observed_max_turns: Arc::new(Mutex::new(Vec::new())),
        prompt_delay: Arc::new(Mutex::new(Duration::ZERO)),
        text_turn_delay: Arc::new(Mutex::new(Duration::ZERO)),
        typed_attempts: Arc::new(Mutex::new(0)),
        prompt_attempts: Arc::new(Mutex::new(0)),
        text_turn_attempts: Arc::new(Mutex::new(0)),
    };

    (
        LlmAgent {
            provider,
            model_id: model_id.to_owned(),
            inner: LlmAgentInner::Mock(inner),
            rate_limiter: None,
        },
        MockLlmAgentController {
            observed_history_lengths,
            observed_history_ptrs,
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

    pub fn provider_id(&self) -> ProviderId {
        self.provider
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Return the rate limiter for this agent's provider, if one is configured.
    pub fn rate_limiter(&self) -> Option<&SharedRateLimiter> {
        self.rate_limiter.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_prompt_delay(&self, delay: Duration) {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            *agent.prompt_delay.lock().unwrap() = delay;
        }
    }

    #[cfg(test)]
    pub(crate) fn push_typed_error(&self, err: TradingError) {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            agent.typed_results.lock().unwrap().push_back(Err(err));
        }
    }

    #[cfg(test)]
    pub(crate) fn push_typed_ok<T>(&self, response: TypedPromptResponse<T>)
    where
        T: Send + 'static,
    {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            agent
                .typed_results
                .lock()
                .unwrap()
                .push_back(Ok(Box::new(response)));
        }
    }

    #[cfg(test)]
    pub(crate) fn typed_attempts(&self) -> usize {
        match &self.inner {
            LlmAgentInner::Mock(agent) => *agent.typed_attempts.lock().unwrap(),
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn prompt_attempts(&self) -> usize {
        match &self.inner {
            LlmAgentInner::Mock(agent) => *agent.prompt_attempts.lock().unwrap(),
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn text_turn_attempts(&self) -> usize {
        match &self.inner {
            LlmAgentInner::Mock(agent) => *agent.text_turn_attempts.lock().unwrap(),
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn observed_max_turns(&self) -> Vec<usize> {
        match &self.inner {
            LlmAgentInner::Mock(agent) => agent.observed_max_turns.lock().unwrap().clone(),
            _ => vec![],
        }
    }

    #[cfg(test)]
    pub(crate) fn set_text_turn_delay(&self, delay: Duration) {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            *agent.text_turn_delay.lock().unwrap() = delay;
        }
    }

    #[cfg(test)]
    pub(crate) fn push_text_turn_error(&self, err: TradingError) {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            agent.text_turn_results.lock().unwrap().push_back(Err(err));
        }
    }

    #[cfg(test)]
    pub(crate) fn push_text_turn_ok(&self, response: PromptResponse) {
        if let LlmAgentInner::Mock(agent) = &self.inner {
            agent
                .text_turn_results
                .lock()
                .unwrap()
                .push_back(Ok(response));
        }
    }

    /// Send a one-shot prompt and return the response text.
    pub async fn prompt(&self, prompt: &str) -> Result<String, PromptError> {
        dispatch_llm_agent!(
            &self.inner,
            |agent| agent.prompt(prompt).await,
            mock = |agent| { Ok(agent.prompt_details(prompt).await?.output) }
        )
    }

    /// Send a one-shot prompt and return text plus aggregated usage details.
    pub async fn prompt_details(&self, prompt: &str) -> Result<PromptResponse, PromptError> {
        dispatch_llm_agent!(
            &self.inner,
            |agent| agent.prompt(prompt).extended_details().await,
            mock = |agent| agent.prompt_details(prompt).await
        )
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

        dispatch_llm_agent!(
            &self.inner,
            |agent| agent
                .prompt_typed::<T>(prompt)
                .max_turns(max_turns)
                .extended_details()
                .await
                .map_err(map_err),
            mock = |mock_agent| mock_agent
                .prompt_typed_details::<T>(prompt, max_turns)
                .await
        )
    }

    /// Send a tool-enabled text prompt and return text plus aggregated usage details.
    ///
    /// Unlike [`prompt_details`] (which is a one-shot call without a tool-turn loop),
    /// this method runs the agent's full multi-turn loop up to `max_turns` so that
    /// tool calls embedded in the prompt are honoured.  The final text output and
    /// provider-reported usage are returned as a [`PromptResponse`].
    ///
    /// This is the production path used by [`super::text_retry::prompt_text_with_retry`].
    pub(crate) async fn prompt_text_details(
        &self,
        prompt: &str,
        max_turns: usize,
    ) -> Result<PromptResponse, TradingError> {
        let map_err = |err| {
            super::error::map_prompt_error_with_context(self.provider_name(), self.model_id(), err)
        };

        // Use PromptRequest with the multi-turn loop to honour tool calls, returning
        // the raw text output and usage without a structured-output parse step.
        dispatch_llm_agent!(
            &self.inner,
            |agent| {
                use rig::agent::PromptRequest;
                PromptRequest::from_agent(agent, prompt)
                    .max_turns(max_turns)
                    .extended_details()
                    .await
                    .map_err(map_err)
            },
            mock = |mock_agent| mock_agent.prompt_text_details(prompt, max_turns).await
        )
    }

    /// Send a prompt with chat history and return the response text.
    pub async fn chat(
        &self,
        prompt: &str,
        chat_history: Vec<Message>,
    ) -> Result<String, PromptError> {
        dispatch_llm_agent!(
            &self.inner,
            |agent| agent.chat(prompt, chat_history).await,
            mock = |agent| {
                let mut history = chat_history;
                Ok(agent.chat_details(prompt, &mut history).await?.output)
            }
        )
    }

    /// Send a prompt with mutable chat history and return response text plus usage details.
    ///
    /// The `chat_history` is updated in place: the new user message and the assistant
    /// response are appended so callers can pass the same `Vec<Message>` across rounds.
    #[allow(clippy::ptr_arg)]
    pub async fn chat_details(
        &self,
        prompt: &str,
        chat_history: &mut Vec<Message>,
    ) -> Result<PromptResponse, PromptError> {
        use rig::agent::PromptRequest;

        let response = dispatch_llm_agent!(
            &self.inner,
            |agent| {
                PromptRequest::from_agent(agent, prompt)
                    .with_history(chat_history.clone())
                    .extended_details()
                    .await
            },
            mock = |agent| agent.chat_details(prompt, chat_history).await
        )?;

        append_response_messages(chat_history, &response);
        Ok(response)
    }
}

/// Append the delta messages from a [`PromptResponse`] to `chat_history`.
///
/// Real providers (OpenAI, Anthropic, etc.) return the round's messages
/// (user prompt + assistant reply) in `response.messages`. This helper extends
/// `chat_history` with those messages so multi-turn callers accumulate context
/// correctly.
///
/// The mock path sets `response.messages = None` and updates history directly inside
/// `MockLlmAgent::chat_details`, so this function is a no-op for mocks.
fn append_response_messages(chat_history: &mut Vec<Message>, response: &PromptResponse) {
    if let Some(messages) = &response.messages {
        chat_history.extend(messages.clone());
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
        *self.prompt_attempts.lock().unwrap() += 1;
        let delay = *self.prompt_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
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
                        cache_creation_input_tokens: 0,
                    },
                ))
            })
    }

    async fn prompt_text_details(
        &self,
        prompt: &str,
        max_turns: usize,
    ) -> Result<PromptResponse, TradingError> {
        self.observed_prompts
            .lock()
            .unwrap()
            .push(prompt.to_owned());
        self.observed_max_turns.lock().unwrap().push(max_turns);
        *self.text_turn_attempts.lock().unwrap() += 1;

        let delay = *self.text_turn_delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let next = self.text_turn_results.lock().unwrap().pop_front();
        match next {
            Some(result) => result,
            None => Ok(mock_prompt_response(
                "",
                rig::completion::Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    total_tokens: 0,
                    cached_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                },
            )),
        }
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
        self.observed_history_ptrs
            .lock()
            .unwrap()
            .push(chat_history.as_ptr() as usize);

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
                        cache_creation_input_tokens: 0,
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

    async fn prompt_typed_details<T>(
        &self,
        prompt: &str,
        _max_turns: usize,
    ) -> Result<TypedPromptResponse<T>, TradingError>
    where
        T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
    {
        self.observed_prompts
            .lock()
            .unwrap()
            .push(prompt.to_owned());
        *self.typed_attempts.lock().unwrap() += 1;

        let next = self.typed_results.lock().unwrap().pop_front();
        match next {
            Some(Ok(response)) => response
                .downcast::<TypedPromptResponse<T>>()
                .map(|response| *response)
                .map_err(|_| {
                    TradingError::Config(anyhow::anyhow!(
                        "typed mock response type mismatch for requested schema"
                    ))
                }),
            Some(Err(err)) => Err(err),
            None => Err(TradingError::Config(anyhow::anyhow!(
                "no typed mock response configured"
            ))),
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
        ($base_builder:expr, $variant:ident) => {{
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
            make_agent!(base, OpenAI)
        }
        ProviderClient::Anthropic(c) => {
            use rig::prelude::CompletionClient;
            let base = c
                .agent(handle.model_id())
                .preamble(system_prompt)
                .max_tokens(4096);
            make_agent!(base, Anthropic)
        }
        ProviderClient::Gemini(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(base, Gemini)
        }
        ProviderClient::Copilot(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(base, Copilot)
        }
        ProviderClient::OpenRouter(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(base, OpenRouter)
        }
        ProviderClient::DeepSeek(c) => {
            use rig::prelude::CompletionClient;
            let base = c.agent(handle.model_id()).preamble(system_prompt);
            make_agent!(base, DeepSeek)
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
    dispatch_llm_agent!(
        &agent.inner,
        |agent_inner| agent_inner.prompt_typed::<T>(prompt).await.map_err(|err| {
            map_structured_output_error_with_context(agent.provider_name(), agent.model_id(), err)
        }),
        mock = |_mock_agent| Err(TradingError::Config(anyhow::anyhow!(
            "typed prompt not supported for mock llm agent"
        )))
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LlmConfig, ProviderSettings};
    use crate::rate_limit::ProviderRateLimiters;
    use crate::state::TradeProposal;
    use crate::{config::ProvidersConfig, providers::ProviderId};
    use rig::tool::Tool;
    use secrecy::SecretString;
    use serde::{Deserialize, Serialize};

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

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestTool;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestToolArgs {}

    impl Tool for TestTool {
        const NAME: &'static str = "test_tool";
        type Error = TradingError;
        type Args = TestToolArgs;
        type Output = String;

        async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
            rig::completion::ToolDefinition {
                name: Self::NAME.to_owned(),
                description: "test tool".to_owned(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            }
        }

        async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok("ok".to_owned())
        }
    }

    // ── Agent builder ────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_agent_creates_openai_agent() {
        let cfg = sample_llm_config();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openai(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "openai");
        assert_eq!(agent.model_id(), "gpt-4o-mini");
        assert!(matches!(&agent.inner, LlmAgentInner::OpenAI(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_anthropic_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "anthropic".to_owned();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_anthropic(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "anthropic");
        assert_eq!(agent.model_id(), "o3");
        assert!(matches!(&agent.inner, LlmAgentInner::Anthropic(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_gemini_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "gemini".to_owned();
        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_gemini(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "gemini");
        assert_eq!(agent.model_id(), "o3");
        assert!(matches!(&agent.inner, LlmAgentInner::Gemini(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_openrouter_agent() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3.6-plus-preview:free".to_owned();

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "openrouter");
        assert_eq!(agent.model_id(), "qwen/qwen3.6-plus-preview:free");
        assert!(matches!(&agent.inner, LlmAgentInner::OpenRouter(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_deepseek_agent() {
        use crate::config::{ProviderSettings, ProvidersConfig};
        use secrecy::SecretString;

        let providers = ProvidersConfig {
            deepseek: ProviderSettings {
                api_key: Some(SecretString::from("test-deepseek-key")),
                base_url: None,
                rpm: 60,
            },
            ..ProvidersConfig::default()
        };

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "deepseek".to_owned();
        cfg.quick_thinking_model = "deepseek-chat".to_owned();

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &providers,
            &crate::rate_limit::ProviderRateLimiters::default(),
        )
        .unwrap();

        let agent = build_agent(&handle, "You are a test agent.");
        assert_eq!(agent.provider_name(), "deepseek");
        assert!(matches!(&agent.inner, LlmAgentInner::DeepSeek(_)));
    }

    #[tokio::test]
    async fn build_agent_creates_openrouter_deep_thinking_agent() {
        let mut cfg = sample_llm_config();
        cfg.deep_thinking_provider = "openrouter".to_owned();
        cfg.deep_thinking_model = "minimax/minimax-m2.5:free".to_owned();

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &cfg,
            &providers_config_with_openrouter(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        let agent = build_agent(&handle, "You are a deep-thinking test agent.");

        assert_eq!(agent.provider_name(), "openrouter");
        assert_eq!(agent.model_id(), "minimax/minimax-m2.5:free");
        assert!(matches!(&agent.inner, LlmAgentInner::OpenRouter(_)));
    }

    #[tokio::test]
    async fn build_agent_propagates_openai_rate_limiter_from_handle() {
        let cfg = sample_llm_config();
        let providers_cfg = ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(SecretString::from("test-key")),
                base_url: None,
                rpm: 60,
            },
            ..ProvidersConfig::default()
        };
        let limiters = ProviderRateLimiters::from_config(&providers_cfg);

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &providers_cfg,
            &limiters,
        )
        .unwrap();
        let expected = handle.rate_limiter().unwrap().label().to_owned();

        let agent = build_agent(&handle, "You are a test agent.");

        assert_eq!(handle.provider_id(), ProviderId::OpenAI);
        assert_eq!(
            agent.rate_limiter().map(|l| l.label()),
            Some(expected.as_str())
        );
    }

    #[tokio::test]
    async fn build_agent_with_tools_preserves_provider_variant_and_rate_limiter() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "openrouter".to_owned();
        cfg.quick_thinking_model = "qwen/qwen3.6-plus-preview:free".to_owned();
        let providers_cfg = ProvidersConfig {
            openrouter: ProviderSettings {
                api_key: Some(SecretString::from("test-openrouter-key")),
                base_url: None,
                rpm: 20,
            },
            ..ProvidersConfig::default()
        };
        let limiters = ProviderRateLimiters::from_config(&providers_cfg);

        let handle = super::super::client::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &cfg,
            &providers_cfg,
            &limiters,
        )
        .unwrap();

        let agent = build_agent_with_tools(
            &handle,
            "You are a tool-using test agent.",
            vec![Box::new(TestTool)],
        );

        assert!(matches!(&agent.inner, LlmAgentInner::OpenRouter(_)));
        assert_eq!(agent.provider_name(), "openrouter");
        assert_eq!(agent.rate_limiter().map(|l| l.label()), Some("openrouter"));
    }

    // ── append_response_messages (TDD – Task A) ──────────────────────────────

    #[test]
    fn append_response_messages_appends_new_messages_to_existing_history() {
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let mut history: Vec<Message> = vec![Message::User {
            content: OneOrMany::one(UserContent::text("prior")),
        }];
        let response = PromptResponse::new(
            "ok",
            Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        )
        .with_messages(vec![
            Message::User {
                content: OneOrMany::one(UserContent::text("next")),
            },
            Message::Assistant {
                content: OneOrMany::one(AssistantContent::text("done")),
                id: None,
            },
        ]);

        append_response_messages(&mut history, &response);

        assert_eq!(history.len(), 3);
    }

    #[test]
    fn append_response_messages_is_noop_when_provider_returns_no_messages() {
        use rig::agent::PromptResponse;
        use rig::completion::Usage;

        let mut history: Vec<Message> = vec![Message::User {
            content: OneOrMany::one(UserContent::text("prior")),
        }];
        let response = PromptResponse::new(
            "ok",
            Usage {
                input_tokens: 0,
                output_tokens: 0,
                total_tokens: 0,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        );

        append_response_messages(&mut history, &response);

        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn mock_agent_supports_typed_prompt_details_for_retry_tests() {
        let (agent, _controller) = mock_llm_agent("o3", vec![], vec![]);
        agent.push_typed_ok(rig::agent::TypedPromptResponse::new(
            TradeProposal {
                action: crate::state::TradeAction::Buy,
                target_price: 123.0,
                stop_loss: 111.0,
                confidence: 0.6,
                rationale: "typed mock".to_owned(),
                valuation_assessment: None,
                scenario_valuation: None,
            },
            rig::completion::Usage {
                input_tokens: 4,
                output_tokens: 2,
                total_tokens: 6,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        ));

        let response = agent
            .prompt_typed_details::<TradeProposal>("prompt", 1)
            .await
            .unwrap();

        assert_eq!(response.output.target_price, 123.0);
        assert_eq!(agent.typed_attempts(), 1);
    }
}
