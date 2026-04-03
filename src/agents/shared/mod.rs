//! Shared internal helpers reused across multiple agent modules.
//!
//! This module is intentionally crate-private so agent-specific prompt builders
//! and validators can stay local while low-level utility logic is centralized.

mod json;
mod prompt;
mod usage;

pub(crate) use json::extract_json_object;
pub(crate) use prompt::{
    UNTRUSTED_CONTEXT_NOTICE, redact_secret_like_values, sanitize_date_for_prompt,
    sanitize_prompt_context, sanitize_symbol_for_prompt, serialize_prompt_value,
};
pub(crate) use usage::agent_token_usage_from_completion;
