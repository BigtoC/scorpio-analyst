//! Global size-cap constants.
//!
//! All values are set to [`usize::MAX`] — no truncation of LLM output or input.

pub const MAX_RATIONALE_CHARS: usize = usize::MAX;
pub const MAX_PROMPT_CONTEXT_CHARS: usize = usize::MAX;
pub const MAX_SUMMARY_CHARS: usize = usize::MAX;
pub const MAX_USER_PROMPT_CHARS: usize = usize::MAX;
pub const MAX_RAW_RESPONSE_CHARS: usize = usize::MAX;
pub const MAX_DEBATE_CHARS: usize = usize::MAX;
pub const MAX_RISK_CHARS: usize = usize::MAX;
pub const MAX_RAW_MODEL_OUTPUT_CHARS: usize = usize::MAX;
pub const MAX_RISK_HISTORY_CHARS: usize = usize::MAX;
pub const NEWS_TITLE_MAX_CHARS: usize = usize::MAX;
pub const NEWS_SNIPPET_MAX_CHARS: usize = usize::MAX;
pub const MACRO_KEYWORD_SCAN_CHARS: usize = usize::MAX;
pub const MAX_ERROR_SUMMARY_CHARS: usize = usize::MAX;
pub const MAX_INDICATOR_NAME_LEN: usize = usize::MAX;
