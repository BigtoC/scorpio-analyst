use std::path::Path;

use anyhow::{Context, Result, bail};
use secrecy::SecretString;
use serde::Deserialize;

/// Top-level application configuration.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub trading: TradingConfig,
    pub api: ApiConfig,
}

/// LLM provider and model routing settings.
#[derive(Debug, Deserialize)]
pub struct LlmConfig {
    pub quick_thinking_provider: String,
    pub deep_thinking_provider: String,
    pub quick_thinking_model: String,
    pub deep_thinking_model: String,
    #[serde(default = "default_debate_rounds")]
    pub max_debate_rounds: u32,
    #[serde(default = "default_risk_rounds")]
    pub max_risk_rounds: u32,
    #[serde(default = "default_agent_timeout")]
    pub agent_timeout_secs: u64,
}

fn default_debate_rounds() -> u32 {
    3
}
fn default_risk_rounds() -> u32 {
    2
}
fn default_agent_timeout() -> u64 {
    30
}

/// Trading-specific parameters.
#[derive(Debug, Deserialize)]
pub struct TradingConfig {
    pub asset_symbol: String,
    #[serde(default)]
    pub backtest_start: Option<String>,
    #[serde(default)]
    pub backtest_end: Option<String>,
}

/// API keys and rate-limit quota settings.
#[derive(Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_finnhub_rate_limit")]
    pub finnhub_rate_limit: u32,

    // Secret keys — loaded from env, not from config.toml
    #[serde(skip)]
    pub openai_api_key: Option<SecretString>,
    #[serde(skip)]
    pub anthropic_api_key: Option<SecretString>,
    #[serde(skip)]
    pub gemini_api_key: Option<SecretString>,
    #[serde(skip)]
    pub finnhub_api_key: Option<SecretString>,
}

fn default_finnhub_rate_limit() -> u32 {
    30
}

// Manual Debug implementation to redact secrets.
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("finnhub_rate_limit", &self.finnhub_rate_limit)
            .field("openai_api_key", &secret_display(&self.openai_api_key))
            .field(
                "anthropic_api_key",
                &secret_display(&self.anthropic_api_key),
            )
            .field("gemini_api_key", &secret_display(&self.gemini_api_key))
            .field("finnhub_api_key", &secret_display(&self.finnhub_api_key))
            .finish()
    }
}

fn secret_display(opt: &Option<SecretString>) -> &str {
    match opt {
        Some(_) => "[REDACTED]",
        None => "<not set>",
    }
}

impl Config {
    /// Load configuration using the 3-tier pipeline:
    /// 1. `config.toml` (defaults)
    /// 2. `.env` via dotenvy (local overrides)
    /// 3. Environment variables (CI/CD overrides)
    pub fn load() -> Result<Self> {
        Self::load_from("config.toml")
    }

    /// Load from a specific config file path (useful for testing).
    pub fn load_from(config_path: impl AsRef<Path>) -> Result<Self> {
        // Layer 2: load .env if present (ignore missing)
        let _ = dotenvy::dotenv();

        // Layer 1 + 3: config.toml base, overridden by SCORPIO_ env vars
        let settings = config::Config::builder()
            .add_source(config::File::from(config_path.as_ref()).required(false))
            .add_source(
                config::Environment::with_prefix("SCORPIO")
                    .separator("_")
                    .try_parsing(true),
            )
            .build()
            .context("failed to build configuration")?;

        let mut cfg: Config = settings
            .try_deserialize()
            .context("failed to deserialize configuration")?;

        // Inject secret keys from environment
        cfg.api.openai_api_key = secret_from_env("SCORPIO_OPENAI_API_KEY");
        cfg.api.anthropic_api_key = secret_from_env("SCORPIO_ANTHROPIC_API_KEY");
        cfg.api.gemini_api_key = secret_from_env("SCORPIO_GEMINI_API_KEY");
        cfg.api.finnhub_api_key = secret_from_env("SCORPIO_FINNHUB_API_KEY");

        cfg.validate()?;
        Ok(cfg)
    }

    /// Fail fast on missing critical settings.
    fn validate(&self) -> Result<()> {
        if self.llm.quick_thinking_provider.is_empty() {
            bail!("config validation: llm.quick_thinking_provider must not be empty");
        }
        if self.llm.deep_thinking_provider.is_empty() {
            bail!("config validation: llm.deep_thinking_provider must not be empty");
        }
        if self.trading.asset_symbol.is_empty() {
            bail!("config validation: trading.asset_symbol must not be empty");
        }
        // Check that at least one LLM key is available
        let has_key = self.api.openai_api_key.is_some()
            || self.api.anthropic_api_key.is_some()
            || self.api.gemini_api_key.is_some();
        if !has_key {
            tracing::warn!(
                "no LLM provider API key found — set SCORPIO_OPENAI_API_KEY, \
                 SCORPIO_ANTHROPIC_API_KEY, or SCORPIO_GEMINI_API_KEY"
            );
        }
        Ok(())
    }
}

fn secret_from_env(key: &str) -> Option<SecretString> {
    std::env::var(key).ok().map(SecretString::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_config_debug_redacts_secrets() {
        let api = ApiConfig {
            finnhub_rate_limit: 30,
            openai_api_key: Some(SecretString::from("super-secret")),
            anthropic_api_key: None,
            gemini_api_key: None,
            finnhub_api_key: None,
        };
        let debug_output = format!("{api:?}");
        assert!(
            debug_output.contains("[REDACTED]"),
            "should redact present keys"
        );
        assert!(
            !debug_output.contains("super-secret"),
            "must not leak secret value"
        );
        assert!(
            debug_output.contains("<not set>"),
            "should mark absent keys"
        );
    }

    #[test]
    fn load_from_defaults_only() {
        // Load only from the checked-in config.toml without any env overrides
        let cfg = Config::load_from("config.toml");
        assert!(
            cfg.is_ok(),
            "loading from config.toml should succeed: {cfg:?}"
        );
        let cfg = cfg.unwrap();
        assert_eq!(cfg.llm.max_debate_rounds, 3);
        assert_eq!(cfg.api.finnhub_rate_limit, 30);
    }
}
