//! Provider factory — construction, validation, agent building, and retry helpers.
//!
//! This module is a **facade**: it re-exports the full public API from focused
//! submodules so that all `crate::providers::factory::` import paths continue to
//! work without modification to any consuming file.
//!
//! ## Submodule responsibilities
//!
//! | Submodule | Responsibility |
//! |-----------|---------------|
//! | [`error`]  | Error mapping (`map_prompt_error`, `map_structured_output_error`) and sanitization utilities (`sanitize_error_summary`, `redact_credentials`, etc.) |
//! | [`client`] | [`ProviderClient`], [`CompletionModelHandle`], [`create_completion_model`], [`preflight_configured_providers`] |
//! | [`agent`]  | [`LlmAgent`], [`build_agent`], [`build_agent_with_tools`], [`prompt_typed`], mock infrastructure |
//! | [`retry`]  | [`RetryOutcome`], all retry/budget loop functions |

mod agent;
mod client;
mod error;
mod retry;

// ── client submodule ─────────────────────────────────────────────────────────

pub use client::{
    CompletionModelHandle, ProviderClient, create_completion_model, create_provider_client,
    preflight_configured_providers,
};

// ── agent submodule ──────────────────────────────────────────────────────────

pub use agent::{LlmAgent, build_agent, build_agent_with_tools, prompt_typed};

// ── retry submodule ──────────────────────────────────────────────────────────

pub use retry::{
    RetryOutcome, chat_with_retry, chat_with_retry_budget, chat_with_retry_details,
    chat_with_retry_details_budget, prompt_typed_with_retry, prompt_with_retry,
    prompt_with_retry_details,
};

// ── error submodule ──────────────────────────────────────────────────────────

pub use error::{map_prompt_error, map_structured_output_error};

pub(crate) use error::sanitize_error_summary;

// ── backward-compat re-export ────────────────────────────────────────────────

pub use super::ProviderId;

// ── test-only mock infrastructure ────────────────────────────────────────────

#[cfg(test)]
pub(crate) use agent::{
    MockChatOutcome, mock_llm_agent, mock_prompt_response,
};
