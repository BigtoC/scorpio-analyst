//! Test-only mock seams for `LlmAgent` to support `text_retry` tests.
//!
//! All types here are gated by `#[cfg(test)]` and live in their own module
//! to keep production code in `agent.rs` clean.

use rig::agent::PromptResponse;

use super::agent::LlmAgent;

/// Create a mock `LlmAgent` pre-loaded with one-shot `PromptResponse` results
/// and returning it alongside a controller for inspection.
///
/// The `prompt_results` queue is consumed by `prompt_details` calls (the one-shot path).
/// Use [`LlmAgent::push_text_turn_ok`] / [`LlmAgent::push_text_turn_error`] for the
/// tool-enabled text-turn path tested by `text_retry`.
pub(crate) fn mock_llm_agent_with_provider(
    model_id: &str,
    prompt_results: Vec<Result<PromptResponse, rig::completion::PromptError>>,
    chat_results: Vec<super::agent::MockChatOutcome>,
) -> (LlmAgent, super::agent::MockLlmAgentController) {
    super::agent::mock_llm_agent(model_id, prompt_results, chat_results)
}

/// Return the number of times the one-shot `prompt_details` path was invoked on
/// the mock agent.  This is the count for `MockLlmAgent::prompt_details`, *not*
/// the tool-turn path.
pub(crate) fn prompt_attempts(agent: &LlmAgent) -> usize {
    agent.prompt_attempts()
}

/// Return the number of times the tool-enabled text-turn path (`prompt_text_details`)
/// was invoked on the mock agent.
pub(crate) fn text_turn_attempts(agent: &LlmAgent) -> usize {
    agent.text_turn_attempts()
}
