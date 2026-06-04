//! Provider factory facade for model handles, agent construction, and retry helpers.
//!
//! The public surface here is intentionally narrow after the factory refactor:
//! callers build a [`CompletionModelHandle`], turn it into an [`LlmAgent`], then use
//! the retry helpers for prompt/chat execution. Lower-level provider IDs live in
//! [`crate::providers::ProviderId`], and error-sanitization details remain internal.
//!
//! ## Submodule responsibilities
//!
//! | Submodule | Responsibility |
//! |-----------|---------------|
//! | [`error`]  | Internal error mapping and sanitization utilities used by the facade |
//! | [`client`] | [`CompletionModelHandle`], [`create_completion_model`], [`create_completion_model_with_copilot`], [`CopilotAuthMode`] |
//! | [`copilot_auth`] | OAuth scope validation, identity binding, and token-cache inspection |
//! | [`agent`]  | [`LlmAgent`], [`build_agent`], [`build_agent_with_tools`], mock infrastructure |
//! | [`retry`]  | [`RetryOutcome`], all retry/budget loop functions |
//! | [`text_retry`] | [`prompt_text_with_retry`] — tool-enabled text prompt with retry |
//! | [`discovery`] | setup-only provider model listing and normalized discovery outcomes |

mod agent;
mod client;
pub mod copilot_auth;
mod discovery;
mod error;
mod retry;
mod text_retry;

// ── client submodule ─────────────────────────────────────────────────────────

pub use client::{
    CompletionModelHandle, CopilotAuthMode, build_copilot_auth_handle, create_completion_model,
    create_completion_model_with_copilot,
};

// ── agent submodule ──────────────────────────────────────────────────────────

pub use agent::{LlmAgent, build_agent, build_agent_with_tools};

// ── retry submodule ──────────────────────────────────────────────────────────

pub use retry::{
    RetryOutcome, chat_with_retry_details, prompt_typed_with_retry,
    prompt_typed_with_retry_validated, prompt_with_retry_validated_details,
    retry_prompt_budget_loop,
};

// ── text_retry submodule ─────────────────────────────────────────────────────

pub use text_retry::{prompt_text_with_retry, prompt_text_with_retry_validated};

pub use discovery::{COPILOT_CURATED_MODELS, ModelDiscoveryOutcome, discover_setup_models};
pub use error::sanitize_error_summary;

// ── test-only mock infrastructure ────────────────────────────────────────────

#[cfg(test)]
pub(crate) use agent::{MockChatOutcome, mock_llm_agent};
