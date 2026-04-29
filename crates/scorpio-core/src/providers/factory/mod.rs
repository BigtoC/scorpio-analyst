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
//! | [`client`] | [`CompletionModelHandle`], [`create_completion_model`] |
//! | [`agent`]  | [`LlmAgent`], [`build_agent`], [`build_agent_with_tools`], [`prompt_typed`], mock infrastructure |
//! | [`retry`]  | [`RetryOutcome`], all retry/budget loop functions |
//! | [`text_retry`] | [`prompt_text_with_retry`] — tool-enabled text prompt with retry |

mod agent;
#[cfg(test)]
pub(crate) mod agent_test_support;
mod client;
mod error;
mod retry;
mod text_retry;

// ── client submodule ─────────────────────────────────────────────────────────

pub use client::{CompletionModelHandle, create_completion_model};

// ── agent submodule ──────────────────────────────────────────────────────────

pub use agent::{LlmAgent, build_agent, build_agent_with_tools, prompt_typed};

// ── retry submodule ──────────────────────────────────────────────────────────

pub use retry::{
    RetryOutcome, chat_with_retry, chat_with_retry_details, prompt_typed_with_retry,
    prompt_with_retry, prompt_with_retry_details,
};

// ── text_retry submodule ─────────────────────────────────────────────────────

pub use text_retry::prompt_text_with_retry;

pub use error::sanitize_error_summary;

// ── test-only mock infrastructure ────────────────────────────────────────────

#[cfg(test)]
pub(crate) use agent::{MockChatOutcome, mock_llm_agent, mock_prompt_response};
