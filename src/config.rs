use std::path::Path;

use anyhow::{Context, Result, bail};
use secrecy::SecretString;
use serde::{Deserialize, Deserializer};

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    pub trading: TradingConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub rate_limits: RateLimitConfig,
}

/// LLM provider and model routing settings.
#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    #[serde(deserialize_with = "deserialize_provider_name")]
    pub quick_thinking_provider: String,
    #[serde(deserialize_with = "deserialize_provider_name")]
    pub deep_thinking_provider: String,
    pub quick_thinking_model: String,
    pub deep_thinking_model: String,
    #[serde(default = "default_debate_rounds")]
    pub max_debate_rounds: u32,
    #[serde(default = "default_risk_rounds")]
    pub max_risk_rounds: u32,
    #[serde(default = "default_agent_timeout", alias = "agent_timeout_secs")]
    pub analyst_timeout_secs: u64,
    /// Maximum number of LLM call retries on transient errors (default: 3).
    #[serde(default = "default_retry_max_retries")]
    pub retry_max_retries: u32,
    /// Base delay in milliseconds for exponential back-off between retries (default: 500).
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
}

/// Validate and normalize an LLM provider name during deserialization.
///
/// Accepts `"openai"`, `"anthropic"`, `"gemini"`, and `"copilot"` (case-insensitive,
/// leading/trailing whitespace ignored). Returns a lower-case canonical form.
/// Unknown values produce a `serde` deserialization error at config-load time,
/// before any provider client is constructed.
fn deserialize_provider_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let canonical = raw.trim().to_ascii_lowercase();
    match canonical.as_str() {
        "openai" | "anthropic" | "gemini" | "copilot" => Ok(canonical),
        unknown => Err(serde::de::Error::custom(format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, copilot)"
        ))),
    }
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
fn default_retry_max_retries() -> u32 {
    3
}
fn default_retry_base_delay_ms() -> u64 {
    500
}

/// Trading-specific parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct TradingConfig {
    pub asset_symbol: String,
    #[serde(default)]
    pub backtest_start: Option<String>,
    #[serde(default)]
    pub backtest_end: Option<String>,
}

/// API keys (loaded from environment, not from config.toml).
#[derive(Clone, Deserialize, Default)]
pub struct ApiConfig {
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

/// Per-provider rate-limit settings.
///
/// All values in requests per minute (RPM) for LLM providers; `finnhub_rps` is
/// requests per second. Setting a value to `0` disables rate limiting for that
/// provider.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// OpenAI requests per minute (0 = disabled).
    #[serde(default = "default_openai_rpm")]
    pub openai_rpm: u32,
    /// Anthropic requests per minute (0 = disabled).
    #[serde(default = "default_anthropic_rpm")]
    pub anthropic_rpm: u32,
    /// Google Gemini requests per minute (0 = disabled).
    #[serde(default = "default_gemini_rpm")]
    pub gemini_rpm: u32,
    /// GitHub Copilot requests per minute (0 = disabled; no documented limit).
    #[serde(default)]
    pub copilot_rpm: u32,
    /// Finnhub requests per second (0 = disabled).
    #[serde(default = "default_finnhub_rps")]
    pub finnhub_rps: u32,
}

fn default_openai_rpm() -> u32 {
    500
}
fn default_anthropic_rpm() -> u32 {
    500
}
fn default_gemini_rpm() -> u32 {
    500
}
fn default_finnhub_rps() -> u32 {
    30
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            openai_rpm: default_openai_rpm(),
            anthropic_rpm: default_anthropic_rpm(),
            gemini_rpm: default_gemini_rpm(),
            copilot_rpm: 0,
            finnhub_rps: default_finnhub_rps(),
        }
    }
}

/// Storage backend settings.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Path to the SQLite snapshot database.
    /// Supports `~/` and `$HOME/` expansion at call-site via [`expand_path`].
    #[serde(default = "default_snapshot_db_path")]
    pub snapshot_db_path: String,
}

fn default_snapshot_db_path() -> String {
    "~/.scorpio-analyst/phase_snapshots.db".to_string()
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            snapshot_db_path: default_snapshot_db_path(),
        }
    }
}

/// Resolve `~/` and `$HOME/` prefix in a path string to the actual home directory.
///
/// - `~/foo` and `$HOME/foo` both expand using the `HOME` environment variable.
/// - If `HOME` is unset, falls back to `"."` with a warning logged via `tracing::warn!`.
/// - All other paths are returned as-is (absolute and relative paths pass through unchanged).
pub fn expand_path(s: &str) -> std::path::PathBuf {
    let suffix = s.strip_prefix("~/").or_else(|| s.strip_prefix("$HOME/"));

    match suffix {
        Some(rest) => {
            let home = std::env::var("HOME").unwrap_or_else(|_| {
                tracing::warn!(
                    "HOME environment variable is not set; \
                     falling back to current directory for path expansion"
                );
                ".".to_string()
            });
            std::path::PathBuf::from(format!("{home}/{rest}"))
        }
        None => std::path::PathBuf::from(s),
    }
}

// Manual Debug implementation to redact secrets.
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
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
                    .separator("__")
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
        // Provider name validity is enforced at deserialization time via
        // `#[serde(deserialize_with = "deserialize_provider_name")]`.
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

    /// Serializes tests that mutate environment variables.
    /// `std::env::set_var` is not thread-safe; all tests touching env vars must
    /// hold this lock for the duration of the test.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn env_override_uses_double_underscore_separator() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK; no other thread mutates env vars concurrently
        unsafe {
            std::env::set_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS", "7");
        }
        let result = Config::load_from("config.toml");
        unsafe {
            std::env::remove_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS");
        }
        let cfg = result.expect("config should load");
        assert_eq!(
            cfg.llm.max_debate_rounds, 7,
            "double-underscore env var should override llm.max_debate_rounds"
        );
    }

    #[test]
    fn api_config_debug_redacts_secrets() {
        let api = ApiConfig {
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
        let _guard = ENV_LOCK.lock().unwrap();
        // Load only from the checked-in config.toml without any env overrides
        let cfg = Config::load_from("config.toml");
        assert!(
            cfg.is_ok(),
            "loading from config.toml should succeed: {cfg:?}"
        );
        let cfg = cfg.unwrap();
        assert_eq!(cfg.llm.max_debate_rounds, 3);
        assert_eq!(cfg.rate_limits.finnhub_rps, 30);
        assert_eq!(cfg.rate_limits.openai_rpm, 500);
        assert_eq!(cfg.rate_limits.anthropic_rpm, 500);
        assert_eq!(cfg.rate_limits.gemini_rpm, 500);
        assert_eq!(cfg.rate_limits.copilot_rpm, 0);
    }

    #[test]
    fn deserialize_provider_name_rejects_unknown() {
        let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("badprovider"));
        assert!(
            result.is_err(),
            "unknown provider names must be rejected at deserialization time"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("badprovider"),
            "error message should include the offending value: {msg}"
        );
    }

    #[test]
    fn deserialize_provider_name_accepts_valid() {
        for name in &["openai", "anthropic", "gemini", "copilot"] {
            let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
                serde::de::value::Error,
            >::new(name));
            assert!(
                result.is_ok(),
                "provider name '{name}' should be accepted: {result:?}"
            );
        }
    }

    #[test]
    fn deserialize_provider_name_normalises_case() {
        let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("  OpenAI  "));
        assert_eq!(result.unwrap(), "openai");
    }

    #[test]
    fn storage_config_defaults_to_tilde_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let cfg = Config::load_from("config.toml").expect("config should load");
        assert_eq!(
            cfg.storage.snapshot_db_path, "~/.scorpio-analyst/phase_snapshots.db",
            "default snapshot_db_path should be the tilde-prefixed path"
        );
    }

    #[test]
    fn storage_config_can_be_overridden_via_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH", "/tmp/custom.db");
        }
        let result = Config::load_from("config.toml");
        unsafe {
            std::env::remove_var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH");
        }
        let cfg = result.expect("config should load");
        assert_eq!(
            cfg.storage.snapshot_db_path, "/tmp/custom.db",
            "env var should override snapshot_db_path"
        );
    }

    #[test]
    fn expand_path_tilde_prefix() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", "/home/testuser") };
        let result = expand_path("~/foo/bar.db");
        // Restore HOME so subsequent tests in other modules are not affected
        unsafe {
            match saved_home {
                Some(ref v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert_eq!(
            result,
            std::path::PathBuf::from("/home/testuser/foo/bar.db")
        );
    }

    #[test]
    fn expand_path_dollar_home_prefix() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", "/home/testuser") };
        let result = expand_path("$HOME/foo/bar.db");
        // Restore HOME so subsequent tests in other modules are not affected
        unsafe {
            match saved_home {
                Some(ref v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert_eq!(
            result,
            std::path::PathBuf::from("/home/testuser/foo/bar.db")
        );
    }

    #[test]
    fn expand_path_absolute_unchanged() {
        // Does not read HOME — no lock needed
        let result = expand_path("/absolute/path.db");
        assert_eq!(result, std::path::PathBuf::from("/absolute/path.db"));
    }

    #[test]
    fn expand_path_relative_unchanged() {
        // Does not read HOME — no lock needed
        let result = expand_path("relative/path.db");
        assert_eq!(result, std::path::PathBuf::from("relative/path.db"));
    }

    #[test]
    fn expand_path_tilde_home_unset_falls_back_to_dot() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::remove_var("HOME") };
        let result = expand_path("~/foo/bar.db");
        // Restore HOME so subsequent tests in other modules are not affected
        unsafe {
            match saved_home {
                Some(ref v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        // Fallback home is "." so format!("{home}/{rest}") == "./foo/bar.db"
        assert_eq!(result, std::path::PathBuf::from("./foo/bar.db"));
    }

    #[test]
    fn expand_path_dollar_home_unset_falls_back_to_dot() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized by ENV_LOCK
        let saved_home = std::env::var("HOME").ok();
        unsafe { std::env::remove_var("HOME") };
        let result = expand_path("$HOME/foo/bar.db");
        // Restore HOME so subsequent tests in other modules are not affected
        unsafe {
            match saved_home {
                Some(ref v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        // Fallback home is "." so format!("{home}/{rest}") == "./foo/bar.db"
        assert_eq!(result, std::path::PathBuf::from("./foo/bar.db"));
    }
}
