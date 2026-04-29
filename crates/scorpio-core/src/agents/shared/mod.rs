//! Shared internal helpers reused across multiple agent modules.
//!
//! This module is intentionally crate-private so agent-specific prompt builders
//! and validators can stay local while low-level utility logic is centralized.

mod json;
mod prompt;
pub(crate) mod technical_projection;
mod usage;
mod valuation_prompt;

pub(crate) use json::extract_json_object;
pub(crate) use prompt::{
    UNTRUSTED_CONTEXT_NOTICE, analysis_emphasis_for_prompt, build_analyst_context_body,
    build_data_quality_context, build_enrichment_context, build_evidence_context,
    build_pack_context, build_thesis_memory_context, redact_secret_like_values,
    sanitize_date_for_prompt, sanitize_prompt_context, sanitize_symbol_for_prompt,
    serialize_prompt_value,
};
pub(crate) use technical_projection::compact_technical_report;
pub(crate) use usage::agent_token_usage_from_completion;
pub(crate) use valuation_prompt::build_valuation_context;
