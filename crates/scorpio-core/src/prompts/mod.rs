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
//!   baseline pack populates these via `include_str!` so the zero-alloc
//!   path remains the default; runtime-loaded packs can opt into
//!   `Cow::Owned` without touching the type.
//! - [`templating::render`] expands `{ticker}` / `{current_date}` /
//!   `{analysis_emphasis}` placeholders using the same `.replace()`
//!   semantics the agent-side prompt builders use today, so extractions
//!   stay byte-identical.
//!
//! Agent modules continue to embed their prompts as `const _SYSTEM_PROMPT`
//! in this slice; migrating those reads to the bundle is the explicit
//! Phase 4 follow-up.
pub mod bundle;
pub mod templating;

pub use bundle::PromptBundle;
pub use templating::render;
