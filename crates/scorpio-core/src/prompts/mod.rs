//! Prompt bundles + templating for pack-driven system prompts.
//!
//! Phase 4 of the asset-class generalization refactor: pack manifests now
//! own the per-role prompt content, so swapping voice per asset class means
//! handing the pipeline a different [`PromptBundle`] instead of forking
//! agent modules.
//!
//! # Scope in this slice
//!
//! - [`PromptBundle`] carries one `Cow<'static, str>` per agent role — the
//!   baseline pack now populates these via `include_str!` under
//!   `analysis_packs/equity/prompts/` so the zero-alloc path remains the
//!   default; runtime-loaded packs can opt into
//!   `Cow::Owned` without touching the type.
//! - [`templating::render`] expands `{ticker}` / `{current_date}` /
//!   `{analysis_emphasis}` placeholders using the same `.replace()`
//!   semantics the agent-side prompt builders use today, so extractions
//!   stay byte-identical.
//!
//! Agent modules still keep `const _SYSTEM_PROMPT` fallbacks for safety, but
//! the active runtime path now prefers the pack-owned bundle slots across the
//! analyst, researcher, trader, risk, and fund-manager agents.
pub mod bundle;
pub mod templating;
mod validation;

pub use bundle::PromptBundle;
pub use templating::render;
pub use validation::{ANALYSIS_EMPHASIS_MAX_LEN, is_effectively_empty, sanitize_analysis_emphasis};
