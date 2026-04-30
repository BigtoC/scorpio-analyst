//! Unified LLM provider layer built on `rig-core`.
//!
//! This module implements dual-tier cognitive routing (quick-thinking vs deep-thinking models)
//! and a provider factory that constructs the correct `rig` client from configuration.
//!
//! # Architecture
//!
//! - [`ModelTier`] encodes the PRD's quick-thinking / deep-thinking routing strategy.
//! - [`factory::create_completion_model`] constructs a tier-specific reusable completion-model handle from config.
//! - [`factory::build_agent`] wraps `rig`'s agent builder with system prompt setup.
//! - [`factory::prompt_with_retry`] and [`factory::chat_with_retry`] add timeout and
//!   exponential backoff retry around `rig` completion calls.
//!
//! # Example
//!
//! ```no_run
//! use scorpio_core::config::Config;
//! use scorpio_core::providers::{ModelTier, factory};
//! use scorpio_core::rate_limit::ProviderRateLimiters;
//!
//! # async fn example() -> Result<(), scorpio_core::error::TradingError> {
//! let cfg = Config::load().expect("config");
//! let handle = factory::create_completion_model(ModelTier::QuickThinking, &cfg.llm, &cfg.providers, &ProviderRateLimiters::default())?;
//! let agent = factory::build_agent(&handle, "You are a fast analyst.");
//! let _model_id = agent.model_id();
//! # Ok(())
//! # }
//! ```

pub mod factory;

use crate::config::LlmConfig;

/// Identifies an LLM provider.
///
/// Used as a key in [`crate::rate_limit::ProviderRateLimiters`] and for provider-specific
/// error messages. Defined here (rather than in `factory`) to avoid a circular dependency
/// between `rate_limit` (which maps provider IDs to limiters) and `factory` (which uses both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    /// OpenRouter API aggregator (300+ models, including free-tier).
    OpenRouter,
    /// DeepSeek API (deepseek-chat, deepseek-reasoner).
    DeepSeek,
}

impl ProviderId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
        }
    }

    pub(crate) const fn missing_key_hint(self) -> &'static str {
        match self {
            Self::OpenAI => "SCORPIO_OPENAI_API_KEY",
            Self::Anthropic => "SCORPIO_ANTHROPIC_API_KEY",
            Self::Gemini => "SCORPIO_GEMINI_API_KEY",
            Self::OpenRouter => "SCORPIO_OPENROUTER_API_KEY",
            Self::DeepSeek => "SCORPIO_DEEPSEEK_API_KEY",
        }
    }
}

/// Cognitive routing tier for model selection.
///
/// The PRD mandates two tiers:
/// - **QuickThinking**: fast, cost-efficient models for analyst data extraction and summaries.
/// - **DeepThinking**: powerful reasoning models for researchers, traders, risk, and fund managers.
///
/// The config is the single source of truth for model IDs — agents never hardcode model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelTier {
    /// Fast, cost-efficient model for analyst tasks (e.g., data extraction, summarisation).
    QuickThinking,
    /// Powerful reasoning model for deep analysis (e.g., research, trading, risk management).
    DeepThinking,
}

impl ModelTier {
    /// Resolve the provider ID string from [`LlmConfig`] based on this tier.
    pub fn provider_id<'a>(&self, config: &'a LlmConfig) -> &'a str {
        match self {
            Self::QuickThinking => &config.quick_thinking_provider,
            Self::DeepThinking => &config.deep_thinking_provider,
        }
    }

    /// Resolve the model ID string from [`LlmConfig`] based on this tier.
    ///
    /// # Example
    ///
    /// ```
    /// use scorpio_core::config::LlmConfig;
    /// use scorpio_core::providers::ModelTier;
    ///
    /// let llm = LlmConfig {
    ///     quick_thinking_provider: "openai".to_owned(),
    ///     deep_thinking_provider: "openai".to_owned(),
    ///     quick_thinking_model: "gpt-4o-mini".to_owned(),
    ///     deep_thinking_model: "o3".to_owned(),
    ///     max_debate_rounds: 3,
    ///     max_risk_rounds: 2,
    ///     analyst_timeout_secs: 30,
    ///     retry_max_retries: 3,
    ///     retry_base_delay_ms: 500,
    /// };
    ///
    /// assert_eq!(ModelTier::QuickThinking.model_id(&llm), "gpt-4o-mini");
    /// assert_eq!(ModelTier::DeepThinking.model_id(&llm), "o3");
    /// ```
    pub fn model_id<'a>(&self, config: &'a LlmConfig) -> &'a str {
        match self {
            Self::QuickThinking => &config.quick_thinking_model,
            Self::DeepThinking => &config.deep_thinking_model,
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuickThinking => write!(f, "quick-thinking"),
            Self::DeepThinking => write!(f, "deep-thinking"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    #[test]
    fn provider_id_resolves_by_tier() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "gemini".to_owned();
        cfg.deep_thinking_provider = "anthropic".to_owned();
        assert_eq!(ModelTier::QuickThinking.provider_id(&cfg), "gemini");
        assert_eq!(ModelTier::DeepThinking.provider_id(&cfg), "anthropic");
    }

    #[test]
    fn model_id_resolves_quick_thinking() {
        let cfg = sample_llm_config();
        assert_eq!(ModelTier::QuickThinking.model_id(&cfg), "gpt-4o-mini");
    }

    #[test]
    fn model_id_resolves_deep_thinking() {
        let cfg = sample_llm_config();
        assert_eq!(ModelTier::DeepThinking.model_id(&cfg), "o3");
    }

    #[test]
    fn model_tier_display() {
        assert_eq!(ModelTier::QuickThinking.to_string(), "quick-thinking");
        assert_eq!(ModelTier::DeepThinking.to_string(), "deep-thinking");
    }

    #[test]
    fn model_tier_equality() {
        assert_eq!(ModelTier::QuickThinking, ModelTier::QuickThinking);
        assert_ne!(ModelTier::QuickThinking, ModelTier::DeepThinking);
    }

    #[test]
    fn model_tier_copy() {
        let tier = ModelTier::DeepThinking;
        let copy = tier;
        assert_eq!(tier, copy);
    }

    #[test]
    fn provider_id_deepseek_exposes_strings_and_missing_key_hint() {
        assert_eq!(ProviderId::DeepSeek.as_str(), "deepseek");
        assert_eq!(ProviderId::DeepSeek.to_string(), "deepseek");
        assert_eq!(
            ProviderId::DeepSeek.missing_key_hint(),
            "SCORPIO_DEEPSEEK_API_KEY"
        );
    }
}
