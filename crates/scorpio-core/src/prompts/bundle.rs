//! [`PromptBundle`] — one `Cow<'static, str>` per agent role.
//!
//! The bundle uses `Cow` so compile-time packs stay zero-alloc
//! (`Cow::Borrowed(include_str!(…))`) while runtime-loaded packs can
//! populate with owned strings (`Cow::Owned(String)`). Per Decision D3
//! in the plan.
use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// Canonical set of prompt slots every pack fills.
///
/// Each slot is the *unmodified* system-prompt template as stored in the
/// pack's `prompts/` directory. Placeholders (`{ticker}`,
/// `{current_date}`, `{analysis_emphasis}`) are expanded at the call site
/// via [`super::templating::render`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptBundle {
    pub fundamental_analyst: Cow<'static, str>,
    pub sentiment_analyst: Cow<'static, str>,
    pub news_analyst: Cow<'static, str>,
    pub technical_analyst: Cow<'static, str>,
    pub bullish_researcher: Cow<'static, str>,
    pub bearish_researcher: Cow<'static, str>,
    pub debate_moderator: Cow<'static, str>,
    pub trader: Cow<'static, str>,
    pub aggressive_risk: Cow<'static, str>,
    pub conservative_risk: Cow<'static, str>,
    pub neutral_risk: Cow<'static, str>,
    pub risk_moderator: Cow<'static, str>,
    pub fund_manager: Cow<'static, str>,
}

impl PromptBundle {
    /// Build a bundle from thirteen static slot values. Useful for tests
    /// and for packs that want to stay zero-alloc.
    ///
    /// The positional signature is intentional — the bundle has one
    /// static slot per agent role and they're all required, so a builder
    /// pattern buys no safety here. Clippy's argument-count lint is
    /// silenced accordingly.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_static(
        fundamental_analyst: &'static str,
        sentiment_analyst: &'static str,
        news_analyst: &'static str,
        technical_analyst: &'static str,
        bullish_researcher: &'static str,
        bearish_researcher: &'static str,
        debate_moderator: &'static str,
        trader: &'static str,
        aggressive_risk: &'static str,
        conservative_risk: &'static str,
        neutral_risk: &'static str,
        risk_moderator: &'static str,
        fund_manager: &'static str,
    ) -> Self {
        Self {
            fundamental_analyst: Cow::Borrowed(fundamental_analyst),
            sentiment_analyst: Cow::Borrowed(sentiment_analyst),
            news_analyst: Cow::Borrowed(news_analyst),
            technical_analyst: Cow::Borrowed(technical_analyst),
            bullish_researcher: Cow::Borrowed(bullish_researcher),
            bearish_researcher: Cow::Borrowed(bearish_researcher),
            debate_moderator: Cow::Borrowed(debate_moderator),
            trader: Cow::Borrowed(trader),
            aggressive_risk: Cow::Borrowed(aggressive_risk),
            conservative_risk: Cow::Borrowed(conservative_risk),
            neutral_risk: Cow::Borrowed(neutral_risk),
            risk_moderator: Cow::Borrowed(risk_moderator),
            fund_manager: Cow::Borrowed(fund_manager),
        }
    }

    /// Placeholder bundle used as the manifest default before Phase 4's
    /// agent migration lands.
    ///
    /// Every slot holds an empty string; agents still read their own
    /// `const _SYSTEM_PROMPT` for now. Once the migration ships the
    /// baseline pack will override this via `include_str!` on the `.md`
    /// files under `analysis_packs/equity/prompts/`.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_static("", "", "", "", "", "", "", "", "", "", "", "", "")
    }
}

impl Default for PromptBundle {
    fn default() -> Self {
        Self::empty()
    }
}
