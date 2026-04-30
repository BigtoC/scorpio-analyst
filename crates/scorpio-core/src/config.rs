use std::path::Path;

use anyhow::{Context, Result};
use secrecy::SecretString;
use serde::{Deserialize, Deserializer};

/// Top-level application configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    #[serde(default)]
    pub trading: TradingConfig,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub rate_limits: RateLimitConfig,
    #[serde(default)]
    pub enrichment: DataEnrichmentConfig,
    /// Selected analysis pack identifier (default: "baseline").
    /// Override: `SCORPIO__ANALYSIS_PACK=baseline`
    #[serde(default = "default_analysis_pack")]
    pub analysis_pack: String,
}

fn default_analysis_pack() -> String {
    "baseline".to_owned()
}

/// Enrichment feature flags and evidence staleness ceiling.
///
/// All flags default to `false`; existing configs without an `[enrichment]`
/// section continue to work with current behaviour unchanged.
#[derive(Debug, Clone, Deserialize)]
pub struct DataEnrichmentConfig {
    /// Whether to fetch earnings-call transcript evidence.
    #[serde(default)]
    pub enable_transcripts: bool,
    /// Whether to fetch analyst consensus estimates evidence.
    #[serde(default)]
    pub enable_consensus_estimates: bool,
    /// Whether to fetch event-driven news evidence.
    #[serde(default)]
    pub enable_event_news: bool,
    /// Maximum age (hours) of cached evidence before it is considered stale.
    #[serde(default = "default_max_evidence_age_hours")]
    pub max_evidence_age_hours: u32,
    /// Per-category fetch timeout (seconds) for enrichment network calls.
    /// Prevents a slow vendor from blocking the entire run.
    #[serde(default = "default_enrichment_fetch_timeout_secs")]
    pub fetch_timeout_secs: u64,
}

fn default_max_evidence_age_hours() -> u32 {
    48
}

fn default_enrichment_fetch_timeout_secs() -> u64 {
    120
}

impl Default for DataEnrichmentConfig {
    fn default() -> Self {
        Self {
            enable_transcripts: false,
            enable_consensus_estimates: false,
            enable_event_news: false,
            max_evidence_age_hours: default_max_evidence_age_hours(),
            fetch_timeout_secs: default_enrichment_fetch_timeout_secs(),
        }
    }
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
    #[serde(default = "default_valuation_fetch_timeout")]
    pub valuation_fetch_timeout_secs: u64,
    /// Maximum number of LLM call retries on transient errors (default: 3).
    #[serde(default = "default_retry_max_retries")]
    pub retry_max_retries: u32,
    /// Base delay in milliseconds for exponential back-off between retries (default: 500).
    #[serde(default = "default_retry_base_delay_ms")]
    pub retry_base_delay_ms: u64,
}

/// Validate and normalize an LLM provider name during deserialization.
///
/// Accepts `"openai"`, `"anthropic"`, `"gemini"`, `"openrouter"`, and `"deepseek"`
/// (case-insensitive, leading/trailing whitespace ignored). Returns a lower-case
/// canonical form. Unknown values produce a `serde` deserialization error at
/// config-load time, before any provider client is constructed.
fn deserialize_provider_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    let canonical = raw.trim().to_ascii_lowercase();
    match canonical.as_str() {
        "openai" | "anthropic" | "gemini" | "openrouter" | "deepseek" => Ok(canonical),
        _unknown => Err(serde::de::Error::custom(format!(
            "unknown LLM provider: \"{_unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek)"
        ))),
    }
}

/// Marker string embedded in the deserialization error for `"copilot"`.
///
/// Used by [`Config::load_from_user_path`] to detect stale copilot routing
/// and surface a friendly recovery message instead of a raw serde error.
pub(crate) const STALE_COPILOT_PROVIDER_MARKER: &str = "unknown LLM provider: \"copilot\"";

fn default_debate_rounds() -> u32 {
    3
}
fn default_risk_rounds() -> u32 {
    2
}
fn default_agent_timeout() -> u64 {
    600
}
fn default_valuation_fetch_timeout() -> u64 {
    30
}
fn default_retry_max_retries() -> u32 {
    3
}
fn default_retry_base_delay_ms() -> u64 {
    500
}

/// Trading-specific parameters.
///
/// `asset_symbol` has been removed — the symbol is now a CLI argument to `scorpio analyze`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TradingConfig {
    #[serde(default)]
    pub backtest_start: Option<String>,
    #[serde(default)]
    pub backtest_end: Option<String>,
}

/// Data API keys (loaded from environment, not from config.toml).
///
/// LLM provider API keys live in [`ProviderSettings`]; this struct holds only
/// keys for non-LLM data services (Finnhub, FRED).
#[derive(Clone, Deserialize, Default)]
pub struct ApiConfig {
    // Secret keys — loaded from env, not from config.toml
    #[serde(skip)]
    pub finnhub_api_key: Option<SecretString>,
    #[serde(skip)]
    pub fred_api_key: Option<SecretString>,
}

/// Per-provider LLM settings: API key, optional base URL override, and rate limit (RPM).
///
/// When `base_url` is `None`, the provider's default endpoint is used (via `rig-core`).
/// When `rpm` is `0`, rate limiting is disabled for that provider.
/// API keys are injected from environment variables (not from config.toml).
#[derive(Clone, Deserialize, Default)]
pub struct ProviderSettings {
    /// API key for this provider (loaded from env, not from config.toml).
    #[serde(skip)]
    pub api_key: Option<SecretString>,
    /// Custom base URL for this provider's API.
    /// When `None`, uses the provider's default endpoint.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Requests per minute (0 = disabled).
    #[serde(default)]
    pub rpm: u32,
}

impl std::fmt::Debug for ProviderSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSettings")
            .field("api_key", &secret_display(&self.api_key))
            .field("base_url", &self.base_url)
            .field("rpm", &self.rpm)
            .finish()
    }
}

/// Nested per-provider configuration: `[providers.<name>]` in config.toml.
///
/// Each field is optional; omitting a provider section entirely uses its defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default = "default_openai_settings")]
    pub openai: ProviderSettings,
    #[serde(default = "default_anthropic_settings")]
    pub anthropic: ProviderSettings,
    #[serde(default = "default_gemini_settings")]
    pub gemini: ProviderSettings,
    #[serde(default = "default_openrouter_settings")]
    pub openrouter: ProviderSettings,
    #[serde(default = "default_deepseek_settings")]
    pub deepseek: ProviderSettings,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            openai: default_openai_settings(),
            anthropic: default_anthropic_settings(),
            gemini: default_gemini_settings(),
            openrouter: default_openrouter_settings(),
            deepseek: default_deepseek_settings(),
        }
    }
}

fn default_openai_settings() -> ProviderSettings {
    ProviderSettings {
        api_key: None,
        base_url: None,
        rpm: 500,
    }
}
fn default_anthropic_settings() -> ProviderSettings {
    ProviderSettings {
        api_key: None,
        base_url: None,
        rpm: 500,
    }
}
fn default_gemini_settings() -> ProviderSettings {
    ProviderSettings {
        api_key: None,
        base_url: None,
        rpm: 500,
    }
}
fn default_openrouter_settings() -> ProviderSettings {
    ProviderSettings {
        api_key: None,
        base_url: None,
        rpm: 20,
    }
}
fn default_deepseek_settings() -> ProviderSettings {
    ProviderSettings {
        api_key: None,
        base_url: None,
        rpm: 60,
    }
}

impl ProvidersConfig {
    /// Look up the settings for a given [`ProviderId`](crate::providers::ProviderId).
    pub fn settings_for(&self, provider: crate::providers::ProviderId) -> &ProviderSettings {
        use crate::providers::ProviderId;
        match provider {
            ProviderId::OpenAI => &self.openai,
            ProviderId::Anthropic => &self.anthropic,
            ProviderId::Gemini => &self.gemini,
            ProviderId::OpenRouter => &self.openrouter,
            ProviderId::DeepSeek => &self.deepseek,
        }
    }

    /// Return the optional base URL override for a given provider.
    pub fn base_url_for(&self, provider: crate::providers::ProviderId) -> Option<&str> {
        self.settings_for(provider).base_url.as_deref()
    }

    /// Return the RPM (requests per minute) for a given provider.
    pub fn rpm_for(&self, provider: crate::providers::ProviderId) -> u32 {
        self.settings_for(provider).rpm
    }

    /// Return the API key for a given provider, if set.
    pub fn api_key_for(&self, provider: crate::providers::ProviderId) -> Option<&SecretString> {
        self.settings_for(provider).api_key.as_ref()
    }
}

/// Data-API rate-limit settings (non-LLM providers).
///
/// LLM provider rate limits have moved to `[providers.<name>.rpm]`.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// Finnhub requests per second (0 = disabled).
    #[serde(default = "default_finnhub_rps")]
    pub finnhub_rps: u32,
    /// FRED requests per second (0 = disabled; free tier allows ~2 rps).
    #[serde(default = "default_fred_rps")]
    pub fred_rps: u32,
    /// Yahoo Finance requests per second (0 = disabled; default: 10).
    #[serde(default = "default_yahoo_finance_rps")]
    pub yahoo_finance_rps: u32,
}

fn default_finnhub_rps() -> u32 {
    30
}
fn default_fred_rps() -> u32 {
    2
}
fn default_yahoo_finance_rps() -> u32 {
    30
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            finnhub_rps: default_finnhub_rps(),
            fred_rps: default_fred_rps(),
            yahoo_finance_rps: default_yahoo_finance_rps(),
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
            .field("finnhub_api_key", &secret_display(&self.finnhub_api_key))
            .field("fred_api_key", &secret_display(&self.fred_api_key))
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
    /// Load configuration from the user-level config file (`~/.scorpio-analyst/config.toml`).
    ///
    /// Precedence (highest wins): env vars > user file > compiled defaults.
    pub fn load() -> Result<Self> {
        match crate::settings::user_config_path() {
            Ok(path) => Self::load_from_user_path(path),
            Err(_) => Self::load_effective_runtime(crate::settings::PartialConfig::default()),
        }
    }

    /// Load configuration from the user-level config file path.
    ///
    /// Loads flat `PartialConfig` from disk, then delegates to
    /// [`Config::load_effective_runtime`] for the shared env/file/default merge.
    ///
    /// If the saved config still routes to the removed Copilot provider, a friendly
    /// error message guides the user to run `scorpio setup`.
    pub fn load_from_user_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let partial = crate::settings::load_user_config_at(path)?;
        let saved_routes_to_copilot = [
            partial.quick_thinking_provider.as_deref(),
            partial.deep_thinking_provider.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|provider| provider.eq_ignore_ascii_case("copilot"));

        match Self::load_effective_runtime(partial) {
            Ok(cfg) => Ok(cfg),
            Err(err)
                if saved_routes_to_copilot
                    && err
                        .chain()
                        .any(|cause| cause.to_string().contains(STALE_COPILOT_PROVIDER_MARKER)) =>
            {
                Err(anyhow::anyhow!(
                    "Your saved configuration still routes to the Copilot provider, which has been removed. \
                     Run `scorpio setup` to update routing to a supported provider."
                ))
            }
            Err(err) => Err(err),
        }
    }

    /// Build the effective runtime config from in-memory wizard/file values.
    ///
    /// Precedence (highest wins): env vars > `partial` > compiled defaults.
    pub fn load_effective_runtime(partial: crate::settings::PartialConfig) -> Result<Self> {
        // Populate process env from .env if present so setup health checks and analyze
        // share the same effective runtime config.
        let _ = dotenvy::dotenv();

        let nested_toml = partial_to_nested_toml_non_secrets(&partial)?;

        let mut cfg: Config = config::Config::builder()
            .add_source(
                config::File::from_str(&nested_toml, config::FileFormat::Toml).required(false),
            )
            .add_source(
                config::Environment::with_prefix("SCORPIO")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .context("failed to build configuration")?
            .try_deserialize()
            .context("failed to deserialize configuration")?;

        // Inject secrets from PartialConfig first.
        if let Some(k) = &partial.openai_api_key {
            cfg.providers.openai.api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.anthropic_api_key {
            cfg.providers.anthropic.api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.gemini_api_key {
            cfg.providers.gemini.api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.openrouter_api_key {
            cfg.providers.openrouter.api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.deepseek_api_key {
            cfg.providers.deepseek.api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.finnhub_api_key {
            cfg.api.finnhub_api_key = Some(SecretString::from(k.clone()));
        }
        if let Some(k) = &partial.fred_api_key {
            cfg.api.fred_api_key = Some(SecretString::from(k.clone()));
        }

        // Env var secrets override file secrets (env wins); warn on collision.
        macro_rules! inject_env_override {
            ($field:expr, $env:literal, $name:literal) => {
                if let Some(key) = secret_from_env($env) {
                    if $field.is_some() {
                        tracing::warn!(
                            provider = $name,
                            env_var = $env,
                            "env var overrides user config file secret"
                        );
                    }
                    $field = Some(key);
                }
            };
        }
        inject_env_override!(
            cfg.providers.openai.api_key,
            "SCORPIO_OPENAI_API_KEY",
            "openai"
        );
        inject_env_override!(
            cfg.providers.anthropic.api_key,
            "SCORPIO_ANTHROPIC_API_KEY",
            "anthropic"
        );
        inject_env_override!(
            cfg.providers.gemini.api_key,
            "SCORPIO_GEMINI_API_KEY",
            "gemini"
        );
        inject_env_override!(
            cfg.providers.openrouter.api_key,
            "SCORPIO_OPENROUTER_API_KEY",
            "openrouter"
        );
        inject_env_override!(
            cfg.providers.deepseek.api_key,
            "SCORPIO_DEEPSEEK_API_KEY",
            "deepseek"
        );
        inject_env_override!(
            cfg.api.finnhub_api_key,
            "SCORPIO_FINNHUB_API_KEY",
            "finnhub"
        );
        inject_env_override!(cfg.api.fred_api_key, "SCORPIO_FRED_API_KEY", "fred");

        cfg.validate()?;
        Ok(cfg)
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
        cfg.providers.openai.api_key = secret_from_env("SCORPIO_OPENAI_API_KEY");
        cfg.providers.anthropic.api_key = secret_from_env("SCORPIO_ANTHROPIC_API_KEY");
        cfg.providers.gemini.api_key = secret_from_env("SCORPIO_GEMINI_API_KEY");
        cfg.api.finnhub_api_key = secret_from_env("SCORPIO_FINNHUB_API_KEY");
        cfg.providers.openrouter.api_key = secret_from_env("SCORPIO_OPENROUTER_API_KEY");
        cfg.providers.deepseek.api_key = secret_from_env("SCORPIO_DEEPSEEK_API_KEY");
        cfg.api.fred_api_key = secret_from_env("SCORPIO_FRED_API_KEY");

        cfg.validate()?;
        Ok(cfg)
    }

    /// Fail fast on missing critical settings.
    fn validate(&self) -> Result<()> {
        // Provider name validity is enforced at deserialization time via
        // `#[serde(deserialize_with = "deserialize_provider_name")]`.
        // Symbol validation has moved to the `cli::analyze` handler (Unit 6).

        // Check that at least one LLM key is available
        if !self.has_any_llm_key() {
            tracing::warn!(
                "no LLM provider API key found — set SCORPIO_OPENAI_API_KEY, \
                 SCORPIO_ANTHROPIC_API_KEY, SCORPIO_GEMINI_API_KEY, SCORPIO_OPENROUTER_API_KEY, \
                 or SCORPIO_DEEPSEEK_API_KEY"
            );
        }

        if self.enrichment.fetch_timeout_secs == 0 {
            anyhow::bail!("fetch_timeout_secs must be at least 1");
        }

        // Validate analysis pack selection before any analysis starts (R6).
        self.analysis_pack
            .parse::<crate::analysis_packs::PackId>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(())
    }

    /// Return `Ok(())` when the effective runtime config can execute an analysis run.
    pub fn is_analysis_ready(&self) -> Result<()> {
        let rate_limiters = crate::rate_limit::ProviderRateLimiters::from_config(&self.providers);

        crate::providers::factory::create_completion_model(
            crate::providers::ModelTier::QuickThinking,
            &self.llm,
            &self.providers,
            &rate_limiters,
        )
        .map_err(|e| anyhow::anyhow!("quick-thinking provider is not ready: {e}"))?;

        crate::providers::factory::create_completion_model(
            crate::providers::ModelTier::DeepThinking,
            &self.llm,
            &self.providers,
            &rate_limiters,
        )
        .map_err(|e| anyhow::anyhow!("deep-thinking provider is not ready: {e}"))?;

        let finnhub_limiter =
            crate::rate_limit::SharedRateLimiter::finnhub_from_config(&self.rate_limits)
                .unwrap_or_else(|| crate::rate_limit::SharedRateLimiter::disabled("finnhub"));
        crate::data::FinnhubClient::new(&self.api, finnhub_limiter)
            .map_err(|e| anyhow::anyhow!("finnhub client is not ready: {e}"))?;

        let fred_limiter =
            crate::rate_limit::SharedRateLimiter::fred_from_config(&self.rate_limits)
                .unwrap_or_else(|| crate::rate_limit::SharedRateLimiter::disabled("fred"));
        crate::data::FredClient::new(&self.api, fred_limiter)
            .map_err(|e| anyhow::anyhow!("fred client is not ready: {e}"))?;

        Ok(())
    }

    fn has_any_llm_key(&self) -> bool {
        self.providers.openai.api_key.is_some()
            || self.providers.anthropic.api_key.is_some()
            || self.providers.gemini.api_key.is_some()
            || self.providers.openrouter.api_key.is_some()
            || self.providers.deepseek.api_key.is_some()
    }

    /// Load only `[providers.*]` settings from a user config file path, ignoring
    /// the `[llm]` routing section entirely.
    ///
    /// This is the setup-safe recovery path: it preserves file-backed provider
    /// overrides (base_url, rpm) plus env overrides and current wizard secrets,
    /// but it does **not** attempt to validate or reuse the current `[llm]`
    /// routing values. A stale `quick_thinking_provider = "copilot"` in the
    /// saved file will not cause a deserialization error here.
    pub fn load_effective_providers_config_from_user_path(
        path: impl AsRef<Path>,
        partial: &crate::settings::PartialConfig,
    ) -> Result<ProvidersConfig> {
        #[derive(Debug, Default, Deserialize)]
        struct ProvidersOnly {
            #[serde(default)]
            providers: ProvidersConfig,
        }

        let _ = dotenvy::dotenv();

        let partial_toml = partial_to_nested_toml_non_secrets(partial)?;

        let settings = config::Config::builder()
            .add_source(config::File::from(path.as_ref()).required(false))
            .add_source(
                config::File::from_str(&partial_toml, config::FileFormat::Toml).required(false),
            )
            .add_source(
                config::Environment::with_prefix("SCORPIO")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .context("failed to build provider-only configuration")?;

        let mut wrapper: ProvidersOnly = settings
            .try_deserialize()
            .context("failed to deserialize provider-only configuration")?;

        apply_partial_provider_secrets(&mut wrapper.providers, partial);
        apply_provider_secret_env_overrides(&mut wrapper.providers);

        Ok(wrapper.providers)
    }
}

/// Synthesise a nested TOML string from the non-secret fields of a `PartialConfig`.
///
/// Only `Some` fields are emitted. The resulting string is fed into `config::File::from_str`
/// so it must match the `Config` serde shape. Secrets are **excluded** — `Config`'s secret
/// fields carry `#[serde(skip)]` and would be silently dropped by the pipeline anyway.
fn partial_to_nested_toml_non_secrets(partial: &crate::settings::PartialConfig) -> Result<String> {
    let mut root = toml::map::Map::new();
    let mut llm = toml::map::Map::new();
    let mut providers = toml::map::Map::new();

    if let Some(p) = &partial.quick_thinking_provider {
        llm.insert(
            "quick_thinking_provider".to_owned(),
            toml::Value::String(p.clone()),
        );
    }
    if let Some(m) = &partial.quick_thinking_model {
        llm.insert(
            "quick_thinking_model".to_owned(),
            toml::Value::String(m.clone()),
        );
    }
    if let Some(p) = &partial.deep_thinking_provider {
        llm.insert(
            "deep_thinking_provider".to_owned(),
            toml::Value::String(p.clone()),
        );
    }
    if let Some(m) = &partial.deep_thinking_model {
        llm.insert(
            "deep_thinking_model".to_owned(),
            toml::Value::String(m.clone()),
        );
    }

    if !llm.is_empty() {
        root.insert("llm".to_owned(), toml::Value::Table(llm));
    }

    let provider_entries = [
        (
            "openai",
            partial.openai_base_url.as_ref(),
            partial.openai_rpm,
        ),
        (
            "anthropic",
            partial.anthropic_base_url.as_ref(),
            partial.anthropic_rpm,
        ),
        (
            "gemini",
            partial.gemini_base_url.as_ref(),
            partial.gemini_rpm,
        ),
        (
            "openrouter",
            partial.openrouter_base_url.as_ref(),
            partial.openrouter_rpm,
        ),
        (
            "deepseek",
            partial.deepseek_base_url.as_ref(),
            partial.deepseek_rpm,
        ),
    ];

    for (name, base_url, rpm) in provider_entries {
        let mut table = toml::map::Map::new();
        if let Some(url) = base_url {
            table.insert("base_url".to_owned(), toml::Value::String(url.clone()));
        }
        if let Some(rpm) = rpm {
            table.insert("rpm".to_owned(), toml::Value::Integer(i64::from(rpm)));
        }
        if !table.is_empty() {
            providers.insert(name.to_owned(), toml::Value::Table(table));
        }
    }

    if !providers.is_empty() {
        root.insert("providers".to_owned(), toml::Value::Table(providers));
    }

    toml::to_string(&toml::Value::Table(root))
        .context("failed to serialize non-secret partial config")
}

fn secret_from_env(key: &str) -> Option<SecretString> {
    std::env::var(key).ok().map(SecretString::from)
}

fn apply_partial_provider_secrets(
    providers: &mut ProvidersConfig,
    partial: &crate::settings::PartialConfig,
) {
    if let Some(k) = &partial.openai_api_key {
        providers.openai.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.anthropic_api_key {
        providers.anthropic.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.gemini_api_key {
        providers.gemini.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.openrouter_api_key {
        providers.openrouter.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.deepseek_api_key {
        providers.deepseek.api_key = Some(SecretString::from(k.clone()));
    }
}

fn apply_provider_secret_env_overrides(providers: &mut ProvidersConfig) {
    inject_provider_env_override(
        &mut providers.openai.api_key,
        "SCORPIO_OPENAI_API_KEY",
        "openai",
    );
    inject_provider_env_override(
        &mut providers.anthropic.api_key,
        "SCORPIO_ANTHROPIC_API_KEY",
        "anthropic",
    );
    inject_provider_env_override(
        &mut providers.gemini.api_key,
        "SCORPIO_GEMINI_API_KEY",
        "gemini",
    );
    inject_provider_env_override(
        &mut providers.openrouter.api_key,
        "SCORPIO_OPENROUTER_API_KEY",
        "openrouter",
    );
    inject_provider_env_override(
        &mut providers.deepseek.api_key,
        "SCORPIO_DEEPSEEK_API_KEY",
        "deepseek",
    );
}

fn inject_provider_env_override(
    field: &mut Option<SecretString>,
    env_var: &str,
    provider_name: &str,
) {
    if let Some(key) = secret_from_env(env_var) {
        if field.is_some() {
            tracing::warn!(
                provider = provider_name,
                env_var,
                "env var overrides user config file secret"
            );
        }
        *field = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    fn sample_config_with_api(api: ApiConfig) -> Config {
        Config {
            llm: LlmConfig {
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
            },
            trading: TradingConfig::default(),
            api,
            providers: ProvidersConfig::default(),
            storage: StorageConfig::default(),
            rate_limits: RateLimitConfig::default(),
            enrichment: DataEnrichmentConfig::default(),
            analysis_pack: default_analysis_pack(),
        }
    }

    /// Serializes tests that mutate environment variables.
    /// `std::env::set_var` is not thread-safe; all tests touching env vars must
    /// hold this lock for the duration of the test.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Minimum valid TOML: only the fields that have no `serde(default)` and
    /// are required by `validate()`. All other fields fall through to their
    /// compiled-in defaults, keeping tests independent of `config.toml`.
    const MINIMAL_CONFIG_TOML: &str = r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#;

    /// Write `content` to a temp file and return `(TempDir, path)`.
    /// The `TempDir` must be kept alive for the duration of the test.
    fn write_config(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir should be created");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, content).expect("config file should be written");
        (dir, path)
    }

    #[test]
    fn env_override_uses_double_underscore_separator() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        // SAFETY: serialized by ENV_LOCK; no other thread mutates env vars concurrently
        unsafe {
            std::env::set_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS", "7");
        }
        let result = Config::load_from(&path);
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
            finnhub_api_key: Some(SecretString::from("ct_finnhub_key")),
            fred_api_key: None,
        };
        let debug_output = format!("{api:?}");
        assert!(
            debug_output.contains("[REDACTED]"),
            "should redact present keys"
        );
        assert!(
            !debug_output.contains("ct_finnhub_key"),
            "must not leak secret value"
        );
        assert!(
            debug_output.contains("<not set>"),
            "should mark absent keys"
        );
        assert!(
            debug_output.contains("finnhub_api_key"),
            "debug output should include finnhub_api_key field"
        );
        assert!(
            debug_output.contains("fred_api_key"),
            "debug output should include fred_api_key field"
        );
    }

    #[test]
    fn load_from_defaults_only() {
        let _guard = ENV_LOCK.lock().unwrap();
        // All values asserted below are compiled-in Rust defaults (serde default fns).
        // MINIMAL_CONFIG_TOML provides only the required fields; everything else falls
        // through to its Default impl so the assertions are independent of config.toml.
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg = Config::load_from(&path).expect("config should load");
        assert_eq!(cfg.llm.max_debate_rounds, 3);
        assert_eq!(cfg.llm.valuation_fetch_timeout_secs, 30);
        assert_eq!(cfg.rate_limits.finnhub_rps, 30);
        assert_eq!(cfg.rate_limits.fred_rps, 2);
        assert_eq!(cfg.rate_limits.yahoo_finance_rps, 30);
        // Provider rpm defaults (default_openai_settings etc.) only activate when a
        // [providers] section is present in TOML; absent the section entirely, serde
        // calls ProvidersConfig::default() which uses ProviderSettings::default() (rpm: 0).
        // Those defaults are covered by the individual env-override tests.
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
        assert!(
            msg.contains("openrouter"),
            "error message should list openrouter among supported providers: {msg}"
        );
    }

    #[test]
    fn deserialize_provider_name_accepts_valid() {
        for name in &["openai", "anthropic", "gemini", "openrouter"] {
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
    fn deserialize_provider_name_normalises_openrouter_case() {
        let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("  OpenRouter  "));
        assert_eq!(result.unwrap(), "openrouter");
    }

    #[test]
    fn load_from_supports_legacy_agent_timeout_secs_alias() {
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
agent_timeout_secs = 45
valuation_fetch_timeout_secs = 9
"#,
        );
        let cfg = Config::load_from(&path).expect("legacy timeout alias should load");
        assert_eq!(cfg.llm.analyst_timeout_secs, 45);
        assert_eq!(cfg.llm.valuation_fetch_timeout_secs, 9);
    }

    #[test]
    fn load_from_supports_canonical_analyst_timeout_secs_key() {
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
analyst_timeout_secs = 60
valuation_fetch_timeout_secs = 12
"#,
        );
        let cfg = Config::load_from(&path).expect("canonical timeout key should load");
        assert_eq!(cfg.llm.analyst_timeout_secs, 60);
        assert_eq!(cfg.llm.valuation_fetch_timeout_secs, 12);
    }

    #[test]
    fn load_from_reads_openrouter_api_key_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        // SAFETY: serialized by ENV_LOCK; no other test sets this var concurrently
        unsafe {
            std::env::set_var("SCORPIO_OPENROUTER_API_KEY", "test-openrouter-key-from-env");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO_OPENROUTER_API_KEY");
        }
        let cfg = result.expect("config should load with openrouter key from env");
        assert_eq!(
            cfg.providers
                .openrouter
                .api_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("test-openrouter-key-from-env")
        );
    }

    #[test]
    fn has_any_llm_key_counts_openrouter_key() {
        let mut cfg = sample_config_with_api(ApiConfig::default());
        cfg.providers.openrouter.api_key = Some(SecretString::from("test-openrouter-key"));
        assert!(cfg.has_any_llm_key());
    }

    #[test]
    fn env_override_supports_openrouter_rate_limit() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        // SAFETY: serialized by ENV_LOCK; no other test sets this var concurrently
        unsafe {
            std::env::set_var("SCORPIO__PROVIDERS__OPENROUTER__RPM", "40");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__PROVIDERS__OPENROUTER__RPM");
        }
        let cfg = result.expect("config should load with openrouter rpm override");
        assert_eq!(cfg.providers.openrouter.rpm, 40);
    }

    #[test]
    fn storage_config_defaults_to_tilde_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg = Config::load_from(&path).expect("config should load");
        assert_eq!(
            cfg.storage.snapshot_db_path, "~/.scorpio-analyst/phase_snapshots.db",
            "default snapshot_db_path should be the tilde-prefixed path"
        );
    }

    #[test]
    fn storage_config_can_be_overridden_via_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__STORAGE__SNAPSHOT_DB_PATH", "/tmp/custom.db");
        }
        let result = Config::load_from(&path);
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
    fn enrichment_fetch_timeout_secs_must_be_positive() {
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"

[enrichment]
fetch_timeout_secs = 0
"#,
        );
        let err = Config::load_from(&path).expect_err("zero timeout should be rejected");
        assert!(
            err.to_string()
                .contains("fetch_timeout_secs must be at least 1"),
            "unexpected error: {err}"
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

    // ── DataEnrichmentConfig tests ────────────────────────────────────────

    #[test]
    fn enrichment_config_defaults_are_all_disabled() {
        let cfg = DataEnrichmentConfig::default();
        assert!(!cfg.enable_transcripts);
        assert!(!cfg.enable_consensus_estimates);
        assert!(!cfg.enable_event_news);
        assert_eq!(cfg.max_evidence_age_hours, 48);
    }

    #[test]
    fn config_loads_enrichment_defaults_from_config_toml() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg = Config::load_from(&path).expect("config should load");
        assert!(!cfg.enrichment.enable_transcripts);
        assert!(!cfg.enrichment.enable_consensus_estimates);
        assert!(!cfg.enrichment.enable_event_news);
        assert_eq!(cfg.enrichment.max_evidence_age_hours, 48);
    }

    #[test]
    fn enrichment_env_override_sets_enable_transcripts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__ENRICHMENT__ENABLE_TRANSCRIPTS", "true");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__ENRICHMENT__ENABLE_TRANSCRIPTS");
        }
        let cfg = result.expect("config should load with enrichment env override");
        assert!(cfg.enrichment.enable_transcripts);
    }

    #[test]
    fn enrichment_env_override_sets_max_evidence_age_hours() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__ENRICHMENT__MAX_EVIDENCE_AGE_HOURS", "24");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__ENRICHMENT__MAX_EVIDENCE_AGE_HOURS");
        }
        let cfg = result.expect("config should load with max_evidence_age_hours override");
        assert_eq!(cfg.enrichment.max_evidence_age_hours, 24);
    }

    #[test]
    fn config_without_enrichment_section_uses_defaults() {
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg = Config::load_from(&path).expect("should load without enrichment section");
        assert!(!cfg.enrichment.enable_transcripts);
        assert!(!cfg.enrichment.enable_consensus_estimates);
        assert!(!cfg.enrichment.enable_event_news);
        assert_eq!(cfg.enrichment.max_evidence_age_hours, 48);
    }

    #[test]
    fn rate_limit_config_default_has_yahoo_finance_rps_30() {
        let cfg = RateLimitConfig::default();
        assert_eq!(
            cfg.yahoo_finance_rps, 30,
            "default yahoo_finance_rps should be 30"
        );
    }

    #[test]
    fn env_override_honours_yahoo_finance_rps() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__RATE_LIMITS__YAHOO_FINANCE_RPS", "5");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__RATE_LIMITS__YAHOO_FINANCE_RPS");
        }
        let cfg = result.expect("config should load with yahoo_finance_rps env override");
        assert_eq!(
            cfg.rate_limits.yahoo_finance_rps, 5,
            "SCORPIO__RATE_LIMITS__YAHOO_FINANCE_RPS env var should override the config value"
        );
    }

    // ── Analysis pack selection tests ────────────────────────────────────

    #[test]
    fn config_defaults_to_baseline_pack() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg = Config::load_from(&path).expect("config should load");
        assert_eq!(
            cfg.analysis_pack, "baseline",
            "default analysis_pack should be 'baseline'"
        );
    }

    #[test]
    fn config_rejects_unknown_pack_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(
            r#"
analysis_pack = "turbo_momentum"

[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
        );
        let err = Config::load_from(&path).expect_err("unknown pack should be rejected");
        assert!(
            err.to_string().contains("unknown analysis pack"),
            "error should mention unknown pack: {err}"
        );
    }

    #[test]
    fn config_accepts_explicit_baseline_pack() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(
            r#"
analysis_pack = "baseline"

[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
        );
        let cfg = Config::load_from(&path).expect("explicit baseline should load");
        assert_eq!(cfg.analysis_pack, "baseline");
    }

    #[test]
    fn config_analysis_pack_overridable_via_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        // SAFETY: serialized by ENV_LOCK
        unsafe {
            std::env::set_var("SCORPIO__ANALYSIS_PACK", "baseline");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__ANALYSIS_PACK");
        }
        let cfg = result.expect("env override for analysis_pack should load");
        assert_eq!(cfg.analysis_pack, "baseline");
    }

    #[test]
    fn config_analysis_pack_env_override_rejects_unknown() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__ANALYSIS_PACK", "nonexistent_pack");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__ANALYSIS_PACK");
        }
        assert!(
            result.is_err(),
            "env-overridden unknown pack should be rejected"
        );
    }

    // ── Config::load_from_user_path tests ────────────────────────────────────

    #[test]
    fn load_from_user_path_populates_llm_routing_from_partial_config() {
        use crate::settings::{PartialConfig, save_user_config_at};
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("anthropic".into()),
            deep_thinking_model: Some("claude-opus-4-5".into()),
            openai_api_key: Some("sk-test".into()),
            ..Default::default()
        };
        save_user_config_at(&partial, &path).unwrap();
        let cfg = Config::load_from_user_path(&path).expect("should load from user path");
        assert_eq!(cfg.llm.quick_thinking_provider, "openai");
        assert_eq!(cfg.llm.deep_thinking_provider, "anthropic");
        assert_eq!(cfg.llm.quick_thinking_model, "gpt-4o-mini");
        assert_eq!(cfg.llm.deep_thinking_model, "claude-opus-4-5");
    }

    #[test]
    fn load_from_user_path_missing_file_succeeds_with_env_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        unsafe {
            std::env::set_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER", "openai");
            std::env::set_var("SCORPIO__LLM__DEEP_THINKING_PROVIDER", "openai");
            std::env::set_var("SCORPIO__LLM__QUICK_THINKING_MODEL", "gpt-4o-mini");
            std::env::set_var("SCORPIO__LLM__DEEP_THINKING_MODEL", "o3");
        }
        let result = Config::load_from_user_path(&path);
        unsafe {
            std::env::remove_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER");
            std::env::remove_var("SCORPIO__LLM__DEEP_THINKING_PROVIDER");
            std::env::remove_var("SCORPIO__LLM__QUICK_THINKING_MODEL");
            std::env::remove_var("SCORPIO__LLM__DEEP_THINKING_MODEL");
        }
        result.expect("missing file should succeed when env vars provide LLM routing");
    }

    #[test]
    fn load_from_user_path_env_override_wins_over_file_value() {
        use crate::settings::{PartialConfig, save_user_config_at};
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-file".into()),
            ..Default::default()
        };
        save_user_config_at(&partial, &path).unwrap();
        unsafe {
            std::env::set_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS", "9");
        }
        let result = Config::load_from_user_path(&path);
        unsafe {
            std::env::remove_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS");
        }
        let cfg = result.expect("config should load");
        assert_eq!(
            cfg.llm.max_debate_rounds, 9,
            "env override must win over compiled default"
        );
    }

    #[test]
    fn load_from_user_path_env_secret_overrides_file_secret() {
        use crate::settings::{PartialConfig, save_user_config_at};
        use secrecy::ExposeSecret;
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-from-file".into()),
            ..Default::default()
        };
        save_user_config_at(&partial, &path).unwrap();
        unsafe {
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "sk-from-env");
        }
        let result = Config::load_from_user_path(&path);
        unsafe {
            std::env::remove_var("SCORPIO_OPENAI_API_KEY");
        }
        let cfg = result.expect("config should load");
        assert_eq!(
            cfg.providers
                .openai
                .api_key
                .as_ref()
                .map(|s| s.expose_secret()),
            Some("sk-from-env"),
            "env var secret must win over file secret"
        );
    }

    #[test]
    fn load_from_user_path_no_trading_section_gives_default_trading_config() {
        use crate::settings::{PartialConfig, save_user_config_at};
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            ..Default::default()
        };
        save_user_config_at(&partial, &path).unwrap();
        let cfg = Config::load_from_user_path(&path).expect("config should load");
        assert_eq!(
            cfg.trading,
            TradingConfig::default(),
            "no trading section should yield TradingConfig::default()"
        );
    }

    #[test]
    fn partial_to_nested_toml_non_secrets_escapes_quotes_and_newlines() {
        let partial = crate::settings::PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt\"4o\nmini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            ..Default::default()
        };

        let nested = partial_to_nested_toml_non_secrets(&partial)
            .expect("non-secret partial config should serialize");
        let parsed: toml::Value = toml::from_str(&nested).expect("generated TOML should parse");

        assert_eq!(
            parsed["llm"]["quick_thinking_model"].as_str(),
            Some("gpt\"4o\nmini"),
            "model value should round-trip as inert data, not new TOML syntax"
        );
        assert!(
            parsed.get("storage").is_none(),
            "model content must not escape into unrelated config sections"
        );
    }

    #[test]
    fn partial_to_nested_toml_non_secrets_includes_provider_overrides() {
        let partial = crate::settings::PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_base_url: Some("https://openai.example.com/v1".into()),
            openai_rpm: Some(123),
            deepseek_base_url: Some("https://deepseek.example.com/v1".into()),
            deepseek_rpm: Some(45),
            ..Default::default()
        };

        let nested = partial_to_nested_toml_non_secrets(&partial)
            .expect("non-secret partial config should serialize");
        let parsed: toml::Value = toml::from_str(&nested).expect("generated TOML should parse");

        assert_eq!(
            parsed["providers"]["openai"]["base_url"].as_str(),
            Some("https://openai.example.com/v1")
        );
        assert_eq!(parsed["providers"]["openai"]["rpm"].as_integer(), Some(123));
        assert_eq!(
            parsed["providers"]["deepseek"]["base_url"].as_str(),
            Some("https://deepseek.example.com/v1")
        );
        assert_eq!(
            parsed["providers"]["deepseek"]["rpm"].as_integer(),
            Some(45)
        );
    }

    // ── Copilot provider removal tests ──────────────────────────────────

    #[test]
    fn deserialize_provider_name_rejects_copilot() {
        let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("copilot"));
        let err = result.expect_err("copilot should no longer be accepted");
        let msg = err.to_string();
        assert!(msg.contains("copilot"));
        assert!(msg.contains("openrouter"));
        assert!(msg.contains("deepseek"));
        assert!(!msg.contains("copilot, openrouter"));
    }

    #[test]
    fn load_from_rejects_copilot_provider_name() {
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "copilot"
deep_thinking_provider = "openai"
quick_thinking_model = "claude-haiku"
deep_thinking_model = "o3"
"#,
        );
        let err = Config::load_from(&path).expect_err("runtime config should reject copilot");
        assert!(
            err.chain().any(|c| c.to_string().contains("copilot")),
            "error chain should mention copilot: {err:#}"
        );
    }

    #[test]
    fn load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(
            r#"
quick_thinking_provider = "copilot"
deep_thinking_provider = "openai"
quick_thinking_model = "claude-haiku"
deep_thinking_model = "o3"
"#,
        );
        let err = Config::load_from_user_path(&path)
            .expect_err("a config that still routes to copilot should fail to load at runtime");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Copilot") || msg.contains("copilot"),
            "expected friendly Copilot reference; got: {msg}"
        );
        assert!(
            msg.contains("scorpio setup"),
            "expected guidance to run setup; got: {msg}"
        );
    }

    #[test]
    fn load_from_user_path_does_not_rewrite_unrelated_copilot_path_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copilot-config.toml");
        std::fs::write(&path, "not valid toml = [").expect("invalid config file should be written");

        let err = Config::load_from_user_path(&path)
            .expect_err("invalid config file should surface its original parse failure");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to parse user config") || msg.contains("TOML parse error"),
            "expected original parse failure; got: {msg}"
        );
        assert!(
            !msg.contains("Run `scorpio setup`"),
            "unrelated copilot mentions must not trigger stale-provider guidance: {msg}"
        );
    }

    #[test]
    fn load_from_user_path_does_not_rewrite_env_override_copilot_errors() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
        );

        let saved_quick = std::env::var("SCORPIO__LLM__QUICK_THINKING_PROVIDER").ok();
        unsafe {
            std::env::set_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER", "copilot");
        }

        let err = Config::load_from_user_path(&path)
            .expect_err("env override should fail without stale-file rewrite");

        unsafe {
            match saved_quick {
                Some(ref value) => {
                    std::env::set_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER", value)
                }
                None => std::env::remove_var("SCORPIO__LLM__QUICK_THINKING_PROVIDER"),
            }
        }

        let msg = format!("{err:#}");
        assert!(
            msg.contains("unknown LLM provider: \"copilot\""),
            "expected raw env override error; got: {msg}"
        );
        assert!(
            !msg.contains("Run `scorpio setup`"),
            "env override failures must not be rewritten as stale saved config: {msg}"
        );
    }

    // ── Symbol validation stubs (relocated to Unit 6 — cli::analyze tests) ──

    /// Symbol-validation tests for `Config::validate()` were removed in Unit 3
    /// because `asset_symbol` moved from config to a CLI argument.
    /// They are re-homed in Unit 6 as `cli::analyze` tests.
    #[test]
    #[ignore = "relocated to cli::analyze tests in Unit 6"]
    fn validate_rejects_empty_symbol() {}

    #[test]
    #[ignore = "relocated to cli::analyze tests in Unit 6"]
    fn validate_rejects_symbol_with_semicolons() {}

    #[test]
    #[ignore = "relocated to cli::analyze tests in Unit 6"]
    fn validate_accepts_lowercase_symbol() {}

    // ── DeepSeek provider tests ───────────────────────────────────────────

    #[test]
    fn deserialize_provider_name_accepts_deepseek() {
        let result = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("deepseek"));
        assert_eq!(result.unwrap(), "deepseek");
    }

    #[test]
    fn deserialize_provider_name_unknown_lists_deepseek() {
        let err = deserialize_provider_name(serde::de::value::StrDeserializer::<
            serde::de::value::Error,
        >::new("badprovider"))
        .unwrap_err();
        assert!(err.to_string().contains("deepseek"));
    }

    #[test]
    fn load_from_reads_deepseek_api_key_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO_DEEPSEEK_API_KEY", "test-deepseek-key-from-env");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO_DEEPSEEK_API_KEY");
        }
        let cfg = result.expect("config should load with deepseek key from env");
        assert_eq!(
            cfg.providers
                .deepseek
                .api_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("test-deepseek-key-from-env")
        );
    }

    #[test]
    fn has_any_llm_key_counts_deepseek_key() {
        let mut cfg = sample_config_with_api(ApiConfig::default());
        cfg.providers.deepseek.api_key = Some(SecretString::from("test-deepseek-key"));
        assert!(cfg.has_any_llm_key());
    }

    #[test]
    fn env_override_supports_deepseek_rate_limit() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        unsafe {
            std::env::set_var("SCORPIO__PROVIDERS__DEEPSEEK__RPM", "45");
        }
        let result = Config::load_from(&path);
        unsafe {
            std::env::remove_var("SCORPIO__PROVIDERS__DEEPSEEK__RPM");
        }
        let cfg = result.expect("config should load with deepseek rpm override");
        assert_eq!(cfg.providers.deepseek.rpm, 45);
    }

    #[test]
    fn load_from_user_path_reads_deepseek_api_key_from_partial_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = crate::settings::PartialConfig {
            deepseek_api_key: Some("deepseek-file-key".into()),
            quick_thinking_provider: Some("deepseek".into()),
            quick_thinking_model: Some("deepseek-chat".into()),
            deep_thinking_provider: Some("deepseek".into()),
            deep_thinking_model: Some("deepseek-reasoner".into()),
            ..Default::default()
        };
        crate::settings::save_user_config_at(&partial, &path).expect("save partial config");
        let cfg = Config::load_from_user_path(&path).expect("load from user path");
        assert_eq!(
            cfg.providers
                .deepseek
                .api_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("deepseek-file-key")
        );
    }

    #[test]
    fn config_without_providers_deepseek_still_deserializes() {
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let cfg =
            Config::load_from(&path).expect("config should load without [providers.deepseek]");
        assert_eq!(cfg.providers.deepseek.rpm, default_deepseek_settings().rpm);
        assert!(cfg.providers.deepseek.api_key.is_none());
    }

    #[test]
    fn load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing()
     {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(
            r#"
[llm]
quick_thinking_provider = "copilot"
deep_thinking_provider = "openai"
quick_thinking_model = "claude-haiku"
deep_thinking_model = "o3"

[providers.deepseek]
base_url = "https://deepseek.example.com/v1"
rpm = 45
"#,
        );
        let partial = crate::settings::PartialConfig {
            openai_api_key: Some("sk-partial-openai".into()),
            ..Default::default()
        };

        // Pre-set the env var so dotenvy::dotenv() (called inside the helper)
        // won't overwrite it with the .env file value.
        let saved_openai_key = std::env::var("SCORPIO_OPENAI_API_KEY").ok();
        unsafe {
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "sk-env-openai");
        }

        let providers = Config::load_effective_providers_config_from_user_path(&path, &partial)
            .expect("provider settings should load without validating stale routing");

        // Restore env so other tests are not affected.
        unsafe {
            match saved_openai_key {
                Some(ref v) => std::env::set_var("SCORPIO_OPENAI_API_KEY", v),
                None => std::env::remove_var("SCORPIO_OPENAI_API_KEY"),
            }
        }

        assert_eq!(
            providers
                .openai
                .api_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("sk-env-openai")
        );
        assert_eq!(
            providers.deepseek.base_url.as_deref(),
            Some("https://deepseek.example.com/v1")
        );
        assert_eq!(providers.deepseek.rpm, 45);
    }

    #[test]
    fn load_effective_providers_config_from_user_path_reads_env_base_url_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let partial = crate::settings::PartialConfig::default();
        unsafe {
            std::env::set_var(
                "SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL",
                "https://deepseek.example.com/v1",
            );
        }

        let result = Config::load_effective_providers_config_from_user_path(&path, &partial);

        unsafe {
            std::env::remove_var("SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL");
        }

        let providers = result.expect("env provider overrides should load");
        assert_eq!(
            providers.deepseek.base_url.as_deref(),
            Some("https://deepseek.example.com/v1")
        );
    }

    #[test]
    fn load_effective_runtime_uses_env_provider_base_url_override_over_partial_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        let partial = crate::settings::PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            deepseek_base_url: Some("https://partial-deepseek.example.com/v1".into()),
            ..Default::default()
        };

        unsafe {
            std::env::set_var(
                "SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL",
                "https://deepseek.example.com/v1",
            );
        }

        let result = Config::load_effective_runtime(partial);

        unsafe {
            std::env::remove_var("SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL");
        }

        let cfg = result.expect("env provider overrides should load");
        assert_eq!(
            cfg.providers.deepseek.base_url.as_deref(),
            Some("https://deepseek.example.com/v1")
        );
    }

    #[test]
    fn load_effective_providers_config_from_user_path_uses_env_base_url_override_over_partial_override()
     {
        let _guard = ENV_LOCK.lock().unwrap();
        let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
        let partial = crate::settings::PartialConfig {
            deepseek_base_url: Some("https://partial-deepseek.example.com/v1".into()),
            ..Default::default()
        };

        unsafe {
            std::env::set_var(
                "SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL",
                "https://deepseek.example.com/v1",
            );
        }

        let result = Config::load_effective_providers_config_from_user_path(&path, &partial);

        unsafe {
            std::env::remove_var("SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL");
        }

        let providers = result.expect("env provider overrides should load");
        assert_eq!(
            providers.deepseek.base_url.as_deref(),
            Some("https://deepseek.example.com/v1")
        );
    }
}
