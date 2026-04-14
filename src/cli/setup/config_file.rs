use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Flat representation of `~/.scorpio-analyst/config.toml` written by `scorpio setup`.
///
/// All fields are `Option<String>` — absent keys in the TOML file remain `None`.
///
/// **Security note:** Secrets are stored as plain `String` (not `SecretString`) because
/// `SecretString` does not implement `Serialize`. Plaintext exists in this struct for the
/// duration of `run_setup` or `Config::load_from_user_path`. It is never forwarded to
/// logging/tracing sinks; the hand-rolled `Debug` impl redacts all `*_api_key` fields.
/// This exposure is broader than the single-line `secret_from_env` path but is accepted
/// because the alternative (custom `Serialize` + `expose_secret`) lands the same plaintext
/// in the serialized TOML string anyway.
#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartialConfig {
    /// Finnhub API key (fundamentals, earnings, company news).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finnhub_api_key: Option<String>,
    /// FRED API key (macro indicators: CPI, inflation, interest rates).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fred_api_key: Option<String>,
    /// OpenAI API key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub openai_api_key: Option<String>,
    /// Anthropic API key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub anthropic_api_key: Option<String>,
    /// Google Gemini API key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gemini_api_key: Option<String>,
    /// OpenRouter API key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub openrouter_api_key: Option<String>,
    /// Quick-thinking LLM provider name (used by analyst agents).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quick_thinking_provider: Option<String>,
    /// Quick-thinking model name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quick_thinking_model: Option<String>,
    /// Deep-thinking LLM provider name (researcher, trader, risk agents).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deep_thinking_provider: Option<String>,
    /// Deep-thinking model name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deep_thinking_model: Option<String>,
}

/// Redacts an `Option<String>` API-key field for `Debug` output.
fn redact(opt: &Option<String>) -> &str {
    match opt {
        Some(_) => "[REDACTED]",
        None => "<not set>",
    }
}

impl std::fmt::Debug for PartialConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartialConfig")
            .field("finnhub_api_key", &redact(&self.finnhub_api_key))
            .field("fred_api_key", &redact(&self.fred_api_key))
            .field("openai_api_key", &redact(&self.openai_api_key))
            .field("anthropic_api_key", &redact(&self.anthropic_api_key))
            .field("gemini_api_key", &redact(&self.gemini_api_key))
            .field("openrouter_api_key", &redact(&self.openrouter_api_key))
            .field("quick_thinking_provider", &self.quick_thinking_provider)
            .field("quick_thinking_model", &self.quick_thinking_model)
            .field("deep_thinking_provider", &self.deep_thinking_provider)
            .field("deep_thinking_model", &self.deep_thinking_model)
            .finish()
    }
}

/// Returns the canonical path for the user-level config file:
/// `~/.scorpio-analyst/config.toml`.
pub fn user_config_path() -> PathBuf {
    crate::config::expand_path("~/.scorpio-analyst/config.toml")
}

/// Load [`PartialConfig`] from `path`.
///
/// Returns [`PartialConfig::default`] if the file does not exist.
/// Returns an error if the file exists but cannot be parsed as TOML.
/// Context strings in errors contain only the file path, never secret values.
pub fn load_user_config_at(path: impl AsRef<Path>) -> anyhow::Result<PartialConfig> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(PartialConfig::default());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("failed to parse config file: {}", path.display()))
}

/// Load [`PartialConfig`] from the default user config path.
///
/// Thin wrapper around [`load_user_config_at`] using [`user_config_path`].
pub fn load_user_config() -> anyhow::Result<PartialConfig> {
    load_user_config_at(user_config_path())
}

/// Write `cfg` atomically to `path`, creating parent directories as needed.
///
/// Uses `NamedTempFile` + rename for atomicity so a partial write never corrupts
/// an existing config. On Unix, `0o600` permissions are set on the temp file
/// *before* the rename to close the race window.
/// Context strings in errors contain only the file path and the underlying I/O error.
pub(crate) fn save_user_config_at(cfg: &PartialConfig, path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!("config path has no parent directory: {}", path.display())
    })?;

    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory: {}", parent.display()))?;

    let toml_str = toml::to_string_pretty(cfg).context("failed to serialize config")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in: {}", parent.display()))?;

    tmp.write_all(toml_str.as_bytes())
        .context("failed to write config bytes to temp file")?;
    tmp.as_file()
        .sync_all()
        .context("failed to sync config temp file to disk")?;

    // Set permissions on the temp file BEFORE persist to avoid a race window
    // where the file is briefly world-readable between rename and chmod.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o600))
            .context("failed to set config file permissions to 0o600")?;
    }

    tmp.persist(path).map_err(|e| {
        anyhow::anyhow!(
            "failed to install config at {}: {}",
            path.display(),
            e.error
        )
    })?;

    Ok(())
}

/// Write `cfg` atomically to the default user config path.
///
/// Thin wrapper around [`save_user_config_at`] using [`user_config_path`].
pub fn save_user_config(cfg: &PartialConfig) -> anyhow::Result<()> {
    save_user_config_at(cfg, user_config_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Serialises tests that mutate `HOME`. Must be held for the full duration
    /// of any test that calls `user_config_path()` / `load_user_config()` /
    /// `save_user_config()` because those expand `~/`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Returns a fully-populated `PartialConfig` with all fields `Some`.
    fn full_partial_config() -> PartialConfig {
        PartialConfig {
            finnhub_api_key: Some("ct_abc123".into()),
            fred_api_key: Some("fred_xyz".into()),
            openai_api_key: Some("sk-openai".into()),
            anthropic_api_key: Some("sk-ant".into()),
            gemini_api_key: Some("AIza_gem".into()),
            openrouter_api_key: Some("or-key".into()),
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("anthropic".into()),
            deep_thinking_model: Some("claude-opus-4-5".into()),
        }
    }

    /// Write a string to `dir/config.toml` and return the path.
    fn write_toml(dir: &tempfile::TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        fs::write(&path, content).expect("write test TOML");
        path
    }

    // ── load_user_config_at ───────────────────────────────────────────────────

    #[test]
    fn load_returns_default_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = load_user_config_at(&path).expect("missing file should return default");
        assert_eq!(result, PartialConfig::default());
    }

    #[test]
    fn load_returns_default_for_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(&dir, "");
        let result = load_user_config_at(&path).expect("empty file should return default");
        assert_eq!(result, PartialConfig::default());
    }

    #[test]
    fn load_returns_error_for_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_toml(&dir, "not [ valid toml ][[[");
        let err = load_user_config_at(&path).expect_err("invalid TOML should error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(path.to_string_lossy().as_ref()),
            "error should mention file path; got: {msg}"
        );
    }

    // ── save_user_config_at + round-trips ─────────────────────────────────────

    #[test]
    fn roundtrip_full_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = full_partial_config();

        save_user_config_at(&original, &path).expect("save should succeed");
        let loaded = load_user_config_at(&path).expect("load should succeed");

        assert_eq!(loaded, original);
    }

    #[test]
    fn roundtrip_partial_config_none_fields_absent_in_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let partial = PartialConfig {
            finnhub_api_key: Some("ct_only".into()),
            ..Default::default()
        };

        save_user_config_at(&partial, &path).expect("save should succeed");

        // Verify the written TOML doesn't contain empty/null entries for None fields
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("finnhub_api_key"), "set field should appear");
        assert!(
            !raw.contains("fred_api_key"),
            "None field should be absent from TOML"
        );
        assert!(
            !raw.contains("openai_api_key"),
            "None field should be absent from TOML"
        );

        let loaded = load_user_config_at(&path).expect("load should succeed");
        assert_eq!(loaded, partial);
    }

    #[test]
    fn save_creates_parent_directory() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialised by ENV_LOCK
        unsafe { std::env::set_var("HOME", home.path()) };

        let result = save_user_config(&full_partial_config());
        unsafe { std::env::remove_var("HOME") };

        result.expect("save should succeed");

        let config_path = home.path().join(".scorpio-analyst/config.toml");
        assert!(config_path.exists(), "config dir + file should be created");
    }

    #[test]
    fn save_returns_error_for_unwritable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("locked");
        fs::create_dir_all(&target_dir).unwrap();

        // Make the directory unwritable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o000)).unwrap();
        }
        #[cfg(not(unix))]
        {
            // Skip on non-Unix: can't reliably simulate unwritable dirs
            return;
        }

        let path = target_dir.join("config.toml");
        let err = save_user_config_at(&full_partial_config(), &path)
            .expect_err("write to unwritable dir should fail");
        assert!(
            format!("{err:#}").contains(target_dir.to_string_lossy().as_ref()),
            "error should mention directory path"
        );

        // Restore permissions so TempDir cleanup can delete it
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target_dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_has_0o600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        save_user_config_at(&full_partial_config(), &path).expect("save should succeed");

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config file should be owner-read/write only (0o600)"
        );
    }

    // ── Debug redaction ───────────────────────────────────────────────────────

    #[test]
    fn debug_redacts_api_key_fields() {
        let cfg = PartialConfig {
            finnhub_api_key: Some("super_secret_finnhub".into()),
            openai_api_key: Some("super_secret_openai".into()),
            quick_thinking_provider: Some("openai".into()),
            ..Default::default()
        };
        let output = format!("{cfg:?}");

        // Secret fields must be redacted
        assert!(
            !output.contains("super_secret_finnhub"),
            "finnhub key must not appear in debug output"
        );
        assert!(
            !output.contains("super_secret_openai"),
            "openai key must not appear in debug output"
        );
        assert!(
            output.contains("[REDACTED]"),
            "present secret fields should show [REDACTED]"
        );
        assert!(
            output.contains("<not set>"),
            "absent secret fields should show <not set>"
        );

        // Non-secret field is visible
        assert!(
            output.contains("openai"),
            "non-secret provider field should be visible"
        );
    }
}
