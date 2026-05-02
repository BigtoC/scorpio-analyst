# GitHub Copilot + Xiaomi MiMo Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-introduce GitHub Copilot (OAuth/device-flow) and add Xiaomi MiMo (API key) as first-class LLM providers using `rig-core 0.36.0`'s native clients, while keeping `create_completion_model(...)` as Scorpio's runtime seam, storing Copilot auth in a Scorpio-owned token directory, and validating cached Copilot auth against GitHub identity, granted scopes, and approved endpoint drift.

**Architecture:** Extend the existing provider seams (`ProviderId`, `ProvidersConfig`, `PartialConfig`, factory client/agent/discovery, rate limiter, setup wizard) without introducing new abstraction layers. `create_completion_model(...)` remains the runtime entrypoint and automatically routes Copilot through `NonInteractiveRuntime`; `step5_health_check` is the only interactive Copilot auth seam and uses an explicit helper with `InteractiveSetup`. Copilot's Scorpio-owned binding records the numeric GitHub account ID plus the approved GitHub API authority derived from rig's `api-key.json`, so runtime can detect account, scope, or endpoint drift. The Copilot token directory under Scorpio's config root is the primary filesystem security boundary: Scorpio owns and verifies the directory, while cache entries must be regular files and are best-effort hardened after auth. Xiaomi MiMo remains a first-class native provider because the approved scope explicitly avoids a generic compatible-provider abstraction; custom `base_url` stays an advanced trusted-host override with structural URL validation, redirects disabled, and explicit operator-facing warnings in setup and docs.

**Tech Stack:** Rust 1.93, `rig-core 0.36.0` (`rig::providers::{copilot, xiaomimimo}`), `secrecy`, `tokio`, `reqwest`, `governor`, `inquire`, `toml`, `url` crate (new dep for base_url validation).

**Spec:** `docs/superpowers/specs/2026-04-30-copilot-xiaomimimo-providers-design.md`

---

## File Structure

| File                                                        | Action | Purpose                                                                                                                                                     |
|-------------------------------------------------------------|--------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `Cargo.toml` (workspace)                                    | Modify | Add `url = "2"` workspace dep                                                                                                                               |
| `crates/scorpio-core/Cargo.toml`                            | Modify | Add `url.workspace = true`                                                                                                                                  |
| `crates/scorpio-core/src/providers/mod.rs`                  | Modify | Add `ProviderId::Copilot`, `ProviderId::XiaomiMimo`                                                                                                         |
| `crates/scorpio-core/src/config.rs`                         | Modify | Accept new provider names; add `[providers.copilot]`/`[providers.xiaomimimo]`; remove stale-Copilot recovery; fix Copilot-only warning path                 |
| `crates/scorpio-core/src/settings.rs`                       | Modify | Add new fields to `PartialConfig`/`UserConfigFile`/`UserConfigProviders`; add `copilot_token_dir()` helper                                                  |
| `crates/scorpio-core/src/providers/factory/client.rs`       | Modify | Add `CopilotAuthMode`, Copilot+XiaomiMimo construction branches, URL validation                                                                             |
| `crates/scorpio-core/src/providers/factory/mod.rs`          | Modify | Re-export Copilot setup helpers, curated-model accessor, and auth helpers for CLI use                                                                       |
| `crates/scorpio-core/src/providers/factory/agent.rs`        | Modify | Add Copilot/XiaomiMimo type aliases, dispatch arms, build branches, token usage handling                                                                    |
| `crates/scorpio-core/src/providers/factory/discovery.rs`    | Modify | Short-circuit Copilot before `list_models()`; add Xiaomi MiMo listing                                                                                       |
| `crates/scorpio-core/src/providers/factory/error.rs`        | Modify | Extend `redact_credentials` with GitHub OAuth token prefixes, device codes, verification URI                                                                |
| `crates/scorpio-core/src/providers/factory/copilot_auth.rs` | Create | Identity-binding record + `GET /user` validation + `api-key.json` authority binding logic                                                                   |
| `crates/scorpio-core/src/app/mod.rs`                        | Modify | Run async Copilot cached-auth revalidation during runtime bootstrap before handle creation                                                                  |
| `crates/scorpio-core/src/rate_limit.rs`                     | Modify | Add Copilot+XiaomiMimo limiter mappings                                                                                                                     |
| `crates/scorpio-cli/src/cli/setup/steps.rs`                 | Modify | Split keyed-provider prompting from routing eligibility; add Copilot-only bypass; update `validate_step3_result`/`providers_with_keys`; Xiaomi MiMo prompts |
| `crates/scorpio-cli/src/cli/setup/model_selection.rs`       | Modify | Copilot static curated list; Xiaomi MiMo discovery; replace `Config::load_effective_runtime` with provider-only load                                        |
| `crates/scorpio-cli/src/cli/setup/mod.rs`                   | Modify | Wire Copilot OAuth health check into `step5_health_check`                                                                                                   |
| `README.md`                                                 | Modify | Re-add Copilot, add Xiaomi MiMo                                                                                                                             |
| `.env.example`                                              | Modify | Add `SCORPIO_XIAOMIMIMO_API_KEY=`                                                                                                                           |

**Commit posture:** The commit steps below are checkpoint suggestions, not a requirement to land 28 final commits. Adjacent tasks within one phase can be squashed into a single integration commit when that keeps the branch easier to review.

---

## Phase 0: Foundation Migration (delete obsolete rejection logic)

This phase removes the temporary "Copilot is removed" recovery path so subsequent phases can add `ProviderId::Copilot` without compile errors.

### Task 1: Remove `STALE_COPILOT_PROVIDER_MARKER` and recovery wrapper

**Files:**
- Modify: `crates/scorpio-core/src/config.rs:122-126` (constant), `crates/scorpio-core/src/config.rs:407-435` (recovery wrapper), tests at `crates/scorpio-core/src/config.rs:1598-1714, 1830-1882`

- [ ] **Step 1: Read the current state of the marker, wrapper, and rejection tests**

```bash
sed -n '120,135p' crates/scorpio-core/src/config.rs
sed -n '405,440p' crates/scorpio-core/src/config.rs
```

- [ ] **Step 2: Delete the `STALE_COPILOT_PROVIDER_MARKER` constant**

Delete lines 122-126 of `crates/scorpio-core/src/config.rs`:

```rust
/// Marker string embedded in the deserialization error for `"copilot"`.
///
/// Used by [`Config::load_from_user_path`] to detect stale copilot routing
/// and surface a friendly recovery message instead of a raw serde error.
pub(crate) const STALE_COPILOT_PROVIDER_MARKER: &str = "unknown LLM provider: \"copilot\"";
```

- [ ] **Step 3: Replace the recovery wrapper in `load_from_user_path` with a plain delegation**

Replace lines 407-435 (the body of `load_from_user_path`) with:

```rust
    /// Load configuration from the user-level config file path.
    ///
    /// Loads flat `PartialConfig` from disk, then delegates to
    /// [`Config::load_effective_runtime`] for the shared env/file/default merge.
    pub fn load_from_user_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let partial = crate::settings::load_user_config_at(path)?;
        Self::load_effective_runtime(partial)
    }
```

- [ ] **Step 4: Delete the six obsolete rejection tests in `config.rs`**

Delete these tests (and the `// ── Copilot provider removal tests ──` header comment):
- `deserialize_provider_name_rejects_copilot` (~line 1601)
- `load_from_rejects_copilot_provider_name` (~line 1614)
- `load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot` (~line 1632)
- `load_from_user_path_does_not_rewrite_unrelated_copilot_path_errors` (~line 1656)
- `load_from_user_path_does_not_rewrite_env_override_copilot_errors` (~line 1676)
- `load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing` (~line 1830)

- [ ] **Step 5: Delete the rejection test in `client.rs`**

Delete `validate_provider_id_rejects_copilot` at `crates/scorpio-core/src/providers/factory/client.rs:649-656`.

- [ ] **Step 6: Delete the rejection test in `settings.rs`**

```bash
grep -n "load_user_config_at_preserves_stale_copilot_routing_strings" crates/scorpio-core/src/settings.rs
```

Delete the test function block found.

- [ ] **Step 7: Delete the rejection test in `model_selection.rs`**

```bash
grep -n "default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported" crates/scorpio-cli/src/cli/setup/model_selection.rs
```

Delete the test function block found.

- [ ] **Step 8: Verify the codebase still compiles**

Run:
```bash
cargo build --workspace
```
Expected: clean build (no references to deleted constants/functions remain).

If `STALE_COPILOT_PROVIDER_MARKER` is referenced elsewhere, search and remove all references:
```bash
grep -rn "STALE_COPILOT_PROVIDER_MARKER" crates/
```

- [ ] **Step 9: Run the full test suite to confirm no regressions**

Run:
```bash
cargo nextest run --workspace --all-features --locked --no-fail-fast
```
Expected: all remaining tests pass.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(providers): remove stale Copilot rejection recovery path

The recovery wrapper, marker constant, and rejection tests existed only to guide
users away from a removed provider. Subsequent commits restore Copilot as a
first-class provider, so this scaffolding is no longer needed.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1: Provider Identity

### Task 2: Add `ProviderId::Copilot` and `ProviderId::XiaomiMimo`

**Files:**
- Modify: `crates/scorpio-core/src/providers/mod.rs:40-69`
- Test: `crates/scorpio-core/src/providers/mod.rs` (existing test module)

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/scorpio-core/src/providers/mod.rs`:

```rust
    #[test]
    fn provider_id_copilot_exposes_strings() {
        assert_eq!(ProviderId::Copilot.as_str(), "copilot");
        assert_eq!(ProviderId::Copilot.to_string(), "copilot");
    }

    #[test]
    fn provider_id_xiaomimimo_exposes_strings_and_missing_key_hint() {
        assert_eq!(ProviderId::XiaomiMimo.as_str(), "xiaomimimo");
        assert_eq!(ProviderId::XiaomiMimo.to_string(), "xiaomimimo");
        assert_eq!(
            ProviderId::XiaomiMimo.missing_key_hint(),
            "SCORPIO_XIAOMIMIMO_API_KEY"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p scorpio-core providers::tests::provider_id_copilot_exposes_strings -- --exact
```
Expected: FAIL with `error[E0599]: no variant or associated item named 'Copilot'`.

- [ ] **Step 3: Add the variants and string mappings**

Update `crates/scorpio-core/src/providers/mod.rs:40-69`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    /// OpenRouter API aggregator (300+ models, including free-tier).
    OpenRouter,
    /// DeepSeek API (deepseek-chat, deepseek-reasoner).
    DeepSeek,
    /// GitHub Copilot via OAuth/device flow (no Scorpio-managed API key).
    Copilot,
    /// Xiaomi MiMo via OpenAI-compatible API.
    XiaomiMimo,
}

impl ProviderId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
            Self::DeepSeek => "deepseek",
            Self::Copilot => "copilot",
            Self::XiaomiMimo => "xiaomimimo",
        }
    }

    pub(crate) const fn missing_key_hint(self) -> &'static str {
        match self {
            Self::OpenAI => "SCORPIO_OPENAI_API_KEY",
            Self::Anthropic => "SCORPIO_ANTHROPIC_API_KEY",
            Self::Gemini => "SCORPIO_GEMINI_API_KEY",
            Self::OpenRouter => "SCORPIO_OPENROUTER_API_KEY",
            Self::DeepSeek => "SCORPIO_DEEPSEEK_API_KEY",
            // Copilot has no key; callers must check the variant before invoking this.
            Self::Copilot => "",
            Self::XiaomiMimo => "SCORPIO_XIAOMIMIMO_API_KEY",
        }
    }
}
```

- [ ] **Step 4: Run the new tests to verify they pass**

```bash
cargo test -p scorpio-core providers::tests::provider_id_copilot_exposes_strings -- --exact
cargo test -p scorpio-core providers::tests::provider_id_xiaomimimo_exposes_strings_and_missing_key_hint -- --exact
```
Expected: both targeted tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/providers/mod.rs
git commit -m "$(cat <<'EOF'
feat(providers): add ProviderId::Copilot and ProviderId::XiaomiMimo

Copilot has no missing-key hint because callers must check the variant
before invoking the function (Copilot uses OAuth, not Scorpio-managed
API keys).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: Configuration

### Task 3: Accept `copilot` and `xiaomimimo` in provider-name deserialization

**Files:**
- Modify: `crates/scorpio-core/src/config.rs:108-119`
- Test: `crates/scorpio-core/src/config.rs` (tests module)

- [ ] **Step 1: Write the failing tests**

Append to the tests module in `crates/scorpio-core/src/config.rs`:

```rust
    #[test]
    fn deserialize_provider_name_accepts_copilot_and_xiaomimimo() {
        let copilot = serde::de::IntoDeserializer::<serde::de::value::Error>::into_deserializer("copilot");
        let result: Result<String, _> = deserialize_provider_name(copilot);
        assert_eq!(result.unwrap(), "copilot");

        let mimo = serde::de::IntoDeserializer::<serde::de::value::Error>::into_deserializer("xiaomimimo");
        let result: Result<String, _> = deserialize_provider_name(mimo);
        assert_eq!(result.unwrap(), "xiaomimimo");
    }

    #[test]
    fn deserialize_provider_name_unknown_error_lists_new_providers() {
        let unknown = serde::de::IntoDeserializer::<serde::de::value::Error>::into_deserializer("nothing");
        let err: serde::de::value::Error = deserialize_provider_name(unknown).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("copilot"), "missing copilot in: {msg}");
        assert!(msg.contains("xiaomimimo"), "missing xiaomimimo in: {msg}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core config::tests::deserialize_provider_name_accepts_copilot_and_xiaomimimo -- --exact
cargo test -p scorpio-core config::tests::deserialize_provider_name_unknown_error_lists_new_providers -- --exact
```
Expected: FAIL with "unknown LLM provider".

- [ ] **Step 3: Update the match arm and error message**

Replace `crates/scorpio-core/src/config.rs:114-118`:

```rust
    match canonical.as_str() {
        "openai" | "anthropic" | "gemini" | "openrouter" | "deepseek" | "copilot"
        | "xiaomimimo" => Ok(canonical),
        _unknown => Err(serde::de::Error::custom(format!(
            "unknown LLM provider: \"{_unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek, copilot, xiaomimimo)"
        ))),
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p scorpio-core config::tests::deserialize_provider_name_accepts_copilot_and_xiaomimimo -- --exact
cargo test -p scorpio-core config::tests::deserialize_provider_name_unknown_error_lists_new_providers -- --exact
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "feat(config): accept copilot and xiaomimimo provider names

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 4: Add `copilot` and `xiaomimimo` sections to `ProvidersConfig`

**Files:**
- Modify: `crates/scorpio-core/src/config.rs:200-292`

- [ ] **Step 1: Write the failing test**

Append to the config tests module:

```rust
    #[test]
    fn providers_config_default_includes_copilot_and_xiaomimimo() {
        let cfg = ProvidersConfig::default();
        // Copilot: rpm conservative, no key, no base_url
        assert!(cfg.copilot.api_key.is_none());
        assert!(cfg.copilot.base_url.is_none());
        assert_eq!(cfg.copilot.rpm, 30);

        // Xiaomi MiMo: rpm conservative, no key, no base_url
        assert!(cfg.xiaomimimo.api_key.is_none());
        assert!(cfg.xiaomimimo.base_url.is_none());
        assert_eq!(cfg.xiaomimimo.rpm, 50);
    }

    #[test]
    fn providers_config_settings_for_resolves_new_providers() {
        let cfg = ProvidersConfig::default();
        assert_eq!(cfg.rpm_for(crate::providers::ProviderId::Copilot), 30);
        assert_eq!(cfg.rpm_for(crate::providers::ProviderId::XiaomiMimo), 50);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core config::tests::providers_config_default_includes_copilot_and_xiaomimimo -- --exact
```
Expected: FAIL with "no field `copilot` on type `ProvidersConfig`".

- [ ] **Step 3: Extend `ProvidersConfig` and `Default`**

Update `crates/scorpio-core/src/config.rs:200-263`:

```rust
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
    #[serde(default = "default_copilot_settings")]
    pub copilot: ProviderSettings,
    #[serde(default = "default_xiaomimimo_settings")]
    pub xiaomimimo: ProviderSettings,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            openai: default_openai_settings(),
            anthropic: default_anthropic_settings(),
            gemini: default_gemini_settings(),
            openrouter: default_openrouter_settings(),
            deepseek: default_deepseek_settings(),
            copilot: default_copilot_settings(),
            xiaomimimo: default_xiaomimimo_settings(),
        }
    }
}

fn default_openai_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 500 }
}
fn default_anthropic_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 500 }
}
fn default_gemini_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 500 }
}
fn default_openrouter_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 20 }
}
fn default_deepseek_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 60 }
}
fn default_copilot_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 30 }
}
fn default_xiaomimimo_settings() -> ProviderSettings {
    ProviderSettings { api_key: None, base_url: None, rpm: 50 }
}
```

- [ ] **Step 4: Update `settings_for` exhaustive match**

Update `crates/scorpio-core/src/config.rs:267-291`:

```rust
impl ProvidersConfig {
    pub fn settings_for(&self, provider: crate::providers::ProviderId) -> &ProviderSettings {
        use crate::providers::ProviderId;
        match provider {
            ProviderId::OpenAI => &self.openai,
            ProviderId::Anthropic => &self.anthropic,
            ProviderId::Gemini => &self.gemini,
            ProviderId::OpenRouter => &self.openrouter,
            ProviderId::DeepSeek => &self.deepseek,
            ProviderId::Copilot => &self.copilot,
            ProviderId::XiaomiMimo => &self.xiaomimimo,
        }
    }

    pub fn base_url_for(&self, provider: crate::providers::ProviderId) -> Option<&str> {
        self.settings_for(provider).base_url.as_deref()
    }

    pub fn rpm_for(&self, provider: crate::providers::ProviderId) -> u32 {
        self.settings_for(provider).rpm
    }

    pub fn api_key_for(&self, provider: crate::providers::ProviderId) -> Option<&SecretString> {
        self.settings_for(provider).api_key.as_ref()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p scorpio-core config::tests::providers_config_default_includes_copilot_and_xiaomimimo -- --exact
cargo test -p scorpio-core config::tests::providers_config_settings_for_resolves_new_providers -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "feat(config): add [providers.copilot] and [providers.xiaomimimo] sections

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 5: Reject `providers.copilot.base_url` at config load time

**Files:**
- Modify: `crates/scorpio-core/src/config.rs` (in the `Config::load_effective_runtime` flow or new validate step)
- Test: same file

- [ ] **Step 1: Locate where `ProvidersConfig` finishes loading**

```bash
grep -n "fn validate\|fn load_effective_runtime\|providers_config_runtime" crates/scorpio-core/src/config.rs | head -10
```

- [ ] **Step 2: Write the failing test**

Append to config tests:

```rust
    #[test]
    fn config_load_rejects_copilot_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, r#"
quick_thinking_provider = "copilot"
deep_thinking_provider = "copilot"
quick_thinking_model = "gpt-4o"
deep_thinking_model = "gpt-4o"

[providers.copilot]
base_url = "https://example.com/v1"
"#).unwrap();

        let err = Config::load_from_user_path(&path).expect_err("copilot base_url must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("copilot") && msg.contains("base_url"),
            "expected copilot base_url rejection, got: {msg}"
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p scorpio-core config::tests::config_load_rejects_copilot_base_url -- --exact
```
Expected: FAIL — config currently accepts the URL.

- [ ] **Step 4: Add the validation**

In `Config::load_effective_runtime` (after the `cfg: Config = ...` build and before returning `Ok(cfg)`), add:

```rust
        if cfg.providers.copilot.base_url.is_some() {
            return Err(anyhow::anyhow!(
                "providers.copilot.base_url is not supported in this slice; \
                 Copilot uses GitHub's API endpoint (or GitHub Enterprise endpoint via api-key.json)"
            ));
        }
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p scorpio-core config::tests::config_load_rejects_copilot_base_url -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "feat(config): reject providers.copilot.base_url at load time

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 6: Add Xiaomi MiMo env-secret loading via `SCORPIO_XIAOMIMIMO_API_KEY`

**Files:**
- Modify: `crates/scorpio-core/src/config.rs` (find where `SCORPIO_DEEPSEEK_API_KEY` is loaded)

- [ ] **Step 1: Find the env-loading pattern for existing providers**

```bash
grep -n "SCORPIO_DEEPSEEK_API_KEY\|apply_provider_secret_env" crates/scorpio-core/src/config.rs | head -10
```

- [ ] **Step 2: Write the failing test**

Append to config tests:

```rust
    #[test]
    fn xiaomimimo_api_key_loads_from_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, r#"
quick_thinking_provider = "xiaomimimo"
deep_thinking_provider = "xiaomimimo"
quick_thinking_model = "mimo-v2.5"
deep_thinking_model = "mimo-v2.5"
"#).unwrap();

        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("SCORPIO_XIAOMIMIMO_API_KEY", "mimo-test-key");
        }
        let cfg = Config::load_from_user_path(&path).expect("config should load");
        assert!(cfg.providers.xiaomimimo.api_key.is_some());
        unsafe {
            std::env::remove_var("SCORPIO_XIAOMIMIMO_API_KEY");
        }
    }
```

Match the existing env-test convention in `crates/scorpio-core/src/config.rs`: take `ENV_LOCK`, then use `std::env::set_var` / `remove_var` inside the locked scope.

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p scorpio-core config::tests::xiaomimimo_api_key_loads_from_env -- --exact
```
Expected: FAIL — env var not yet wired.

- [ ] **Step 4: Wire the env-secret loading**

Find the function (e.g., `apply_provider_secret_env_overrides`) that maps env vars to `cfg.providers.<provider>.api_key`. Add the case for Xiaomi MiMo, mirroring DeepSeek:

```rust
    if let Ok(key) = std::env::var("SCORPIO_XIAOMIMIMO_API_KEY") {
        if !key.is_empty() {
            cfg.providers.xiaomimimo.api_key = Some(SecretString::from(key));
        }
    }
```

If there is a precedence-collision warning path (`tracing::warn!` on env vs file conflict), mirror the DeepSeek branch verbatim.

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p scorpio-core config::tests::xiaomimimo_api_key_loads_from_env -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "feat(config): load SCORPIO_XIAOMIMIMO_API_KEY from environment

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 7: Skip "no LLM key found" warning for Copilot-only routing

**Files:**
- Modify: `crates/scorpio-core/src/config.rs` (in `validate()` or `has_any_llm_key()`)

- [ ] **Step 1: Find the warning emission**

```bash
grep -n "no LLM provider API key found\|has_any_llm_key" crates/scorpio-core/src/config.rs
```

- [ ] **Step 2: Write the failing test**

Append to config tests:

```rust
    #[test]
    fn validate_does_not_warn_for_copilot_only_routing() {
        // Capture tracing output
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, r#"
quick_thinking_provider = "copilot"
deep_thinking_provider = "copilot"
quick_thinking_model = "gpt-4o"
deep_thinking_model = "gpt-4o"
"#).unwrap();

        // For now we test the helper function directly:
        let cfg = Config::load_from_user_path(&path).expect("config loads");
        // The internal helper deciding whether to warn should report false for Copilot-only.
        assert!(!cfg.should_warn_no_llm_key(),
            "Copilot-only routing should not produce a missing-key warning");
    }
```

(The helper `should_warn_no_llm_key` is being introduced here — adjust naming if a similar helper already exists.)

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p scorpio-core config::tests::validate_does_not_warn_for_copilot_only_routing -- --exact
```
Expected: FAIL — method doesn't exist.

- [ ] **Step 4: Refactor the warning gate into a helper**

In `Config`'s impl block:

```rust
    /// Whether `validate()` should emit the "no LLM provider API key found" warning.
    ///
    /// Returns `false` when both routing tiers are `copilot` (which uses OAuth, not API keys).
    pub fn should_warn_no_llm_key(&self) -> bool {
        let copilot_only = self.llm.quick_thinking_provider == "copilot"
            && self.llm.deep_thinking_provider == "copilot";
        if copilot_only {
            return false;
        }
        !self.has_any_llm_key()
    }
```

Then update the existing warn site (in `validate()`) to use `if self.should_warn_no_llm_key() { ... }`.

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p scorpio-core config::tests::validate_does_not_warn_for_copilot_only_routing -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "fix(config): skip missing-key warning for Copilot-only routing

Copilot uses OAuth, not Scorpio-managed API keys, so the existing
\"no LLM provider API key found\" warning is misleading when both
routing tiers point to copilot.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 3: Persisted Setup Boundary

### Task 8: Extend `PartialConfig` with new fields

**Files:**
- Modify: `crates/scorpio-core/src/settings.rs:19-65` (`UserConfigFile`), `67-79` (`UserConfigProviders`), `217-282` (`PartialConfig`), `101-183` (conversions), `292-340` (Debug impl)

- [ ] **Step 1: Write the failing test**

Append to `crates/scorpio-core/src/settings.rs` tests:

```rust
    #[test]
    fn partial_config_round_trips_xiaomimimo_secret_and_copilot_rpm() {
        let mut p = PartialConfig::default();
        p.xiaomimimo_api_key = Some("mimo-secret".to_owned());
        p.xiaomimimo_base_url = Some("https://api.xiaomimimo.com/v1".to_owned());
        p.xiaomimimo_rpm = Some(75);
        p.copilot_rpm = Some(60);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_user_config_at(&path, &p).expect("save");
        let loaded = load_user_config_at(&path).expect("load");

        assert_eq!(loaded.xiaomimimo_api_key.as_deref(), Some("mimo-secret"));
        assert_eq!(loaded.xiaomimimo_base_url.as_deref(), Some("https://api.xiaomimimo.com/v1"));
        assert_eq!(loaded.xiaomimimo_rpm, Some(75));
        assert_eq!(loaded.copilot_rpm, Some(60));
    }

    #[test]
    fn partial_config_debug_redacts_xiaomimimo_secret() {
        let mut p = PartialConfig::default();
        p.xiaomimimo_api_key = Some("mimo-secret-123".to_owned());
        let dbg = format!("{p:?}");
        assert!(!dbg.contains("mimo-secret-123"), "raw secret leaked: {dbg}");
        assert!(dbg.contains("xiaomimimo_api_key"));
    }

    #[test]
    fn partial_config_serializes_xiaomimimo_under_providers_table() {
        let mut p = PartialConfig::default();
        p.xiaomimimo_base_url = Some("https://api.xiaomimimo.com/v1".to_owned());
        p.xiaomimimo_rpm = Some(75);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save_user_config_at(&path, &p).expect("save");
        let raw = std::fs::read_to_string(&path).expect("read");
        assert!(raw.contains("[providers.xiaomimimo]"),
            "expected nested table, got:\n{raw}");
        assert!(!raw.contains("xiaomimimo_base_url ="),
            "must not use legacy flat format");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core settings::tests::partial_config_round_trips_xiaomimimo_secret_and_copilot_rpm -- --exact
cargo test -p scorpio-core settings::tests::partial_config_debug_redacts_xiaomimimo_secret -- --exact
cargo test -p scorpio-core settings::tests::partial_config_serializes_xiaomimimo_under_providers_table -- --exact
```
Expected: FAIL with "no field `xiaomimimo_api_key`".

- [ ] **Step 3: Add fields to `PartialConfig`**

Append to `PartialConfig` (around line 282 of `crates/scorpio-core/src/settings.rs`):

```rust
    /// Xiaomi MiMo API key.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub xiaomimimo_api_key: Option<String>,
    /// Optional Xiaomi MiMo base URL override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub xiaomimimo_base_url: Option<String>,
    /// Optional Xiaomi MiMo requests-per-minute override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub xiaomimimo_rpm: Option<u32>,
    /// Optional GitHub Copilot requests-per-minute override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub copilot_rpm: Option<u32>,
```

- [ ] **Step 4: Add fields to `UserConfigFile` and `UserConfigProviders`**

Update `UserConfigFile` (lines 19-65) by appending this top-level secret field:

```rust
    #[serde(skip_serializing_if = "Option::is_none", default)]
    xiaomimimo_api_key: Option<String>,
```

Update `UserConfigProviders` (lines 67-79) by appending:

```rust
    #[serde(default, skip_serializing_if = "UserConfigProvider::is_empty")]
    copilot: UserConfigProvider,
    #[serde(default, skip_serializing_if = "UserConfigProvider::is_empty")]
    xiaomimimo: UserConfigProvider,
```

- [ ] **Step 5: Update `From<UserConfigFile> for PartialConfig`**

Update `crates/scorpio-core/src/settings.rs:101-133` to add the new fields:

```rust
impl From<UserConfigFile> for PartialConfig {
    fn from(value: UserConfigFile) -> Self {
        let openai = value.providers.openai;
        let anthropic = value.providers.anthropic;
        let gemini = value.providers.gemini;
        let openrouter = value.providers.openrouter;
        let deepseek = value.providers.deepseek;
        let copilot = value.providers.copilot;
        let xiaomimimo = value.providers.xiaomimimo;

        Self {
            finnhub_api_key: value.finnhub_api_key,
            fred_api_key: value.fred_api_key,
            openai_api_key: value.openai_api_key,
            anthropic_api_key: value.anthropic_api_key,
            gemini_api_key: value.gemini_api_key,
            openrouter_api_key: value.openrouter_api_key,
            deepseek_api_key: value.deepseek_api_key,
            xiaomimimo_api_key: value.xiaomimimo_api_key,
            quick_thinking_provider: value.quick_thinking_provider,
            quick_thinking_model: value.quick_thinking_model,
            deep_thinking_provider: value.deep_thinking_provider,
            deep_thinking_model: value.deep_thinking_model,
            openai_base_url: openai.base_url.or(value.openai_base_url),
            anthropic_base_url: anthropic.base_url.or(value.anthropic_base_url),
            gemini_base_url: gemini.base_url.or(value.gemini_base_url),
            openrouter_base_url: openrouter.base_url.or(value.openrouter_base_url),
            deepseek_base_url: deepseek.base_url.or(value.deepseek_base_url),
            xiaomimimo_base_url: xiaomimimo.base_url,
            openai_rpm: openai.rpm.or(value.openai_rpm),
            anthropic_rpm: anthropic.rpm.or(value.anthropic_rpm),
            gemini_rpm: gemini.rpm.or(value.gemini_rpm),
            openrouter_rpm: openrouter.rpm.or(value.openrouter_rpm),
            deepseek_rpm: deepseek.rpm.or(value.deepseek_rpm),
            xiaomimimo_rpm: xiaomimimo.rpm,
            copilot_rpm: copilot.rpm,
        }
    }
}
```

- [ ] **Step 6: Update `From<&PartialConfig> for UserConfigFile`**

Update lines 135-183 to populate the new non-secret provider overrides under the nested table while keeping `xiaomimimo_api_key` as a top-level secret field, matching the existing OpenAI/Anthropic/Gemini/OpenRouter/DeepSeek secret layout:

```rust
impl From<&PartialConfig> for UserConfigFile {
    fn from(value: &PartialConfig) -> Self {
        Self {
            finnhub_api_key: value.finnhub_api_key.clone(),
            fred_api_key: value.fred_api_key.clone(),
            openai_api_key: value.openai_api_key.clone(),
            anthropic_api_key: value.anthropic_api_key.clone(),
            gemini_api_key: value.gemini_api_key.clone(),
            openrouter_api_key: value.openrouter_api_key.clone(),
            deepseek_api_key: value.deepseek_api_key.clone(),
            xiaomimimo_api_key: value.xiaomimimo_api_key.clone(),
            quick_thinking_provider: value.quick_thinking_provider.clone(),
            quick_thinking_model: value.quick_thinking_model.clone(),
            deep_thinking_provider: value.deep_thinking_provider.clone(),
            deep_thinking_model: value.deep_thinking_model.clone(),
            providers: UserConfigProviders {
                openai: UserConfigProvider {
                    base_url: value.openai_base_url.clone(),
                    rpm: value.openai_rpm,
                },
                anthropic: UserConfigProvider {
                    base_url: value.anthropic_base_url.clone(),
                    rpm: value.anthropic_rpm,
                },
                gemini: UserConfigProvider {
                    base_url: value.gemini_base_url.clone(),
                    rpm: value.gemini_rpm,
                },
                openrouter: UserConfigProvider {
                    base_url: value.openrouter_base_url.clone(),
                    rpm: value.openrouter_rpm,
                },
                deepseek: UserConfigProvider {
                    base_url: value.deepseek_base_url.clone(),
                    rpm: value.deepseek_rpm,
                },
                copilot: UserConfigProvider {
                    base_url: None,
                    rpm: value.copilot_rpm,
                },
                xiaomimimo: UserConfigProvider {
                    base_url: value.xiaomimimo_base_url.clone(),
                    rpm: value.xiaomimimo_rpm,
                },
            },
            openai_base_url: None,
            anthropic_base_url: None,
            gemini_base_url: None,
            openrouter_base_url: None,
            deepseek_base_url: None,
            openai_rpm: None,
            anthropic_rpm: None,
            gemini_rpm: None,
            openrouter_rpm: None,
            deepseek_rpm: None,
        }
    }
}
```

- [ ] **Step 7: Update the redacted `Debug` impl for `PartialConfig`**

In the `impl std::fmt::Debug for PartialConfig` block (around line 292-340), add a field for `xiaomimimo_api_key` mirroring the other secret redactions:

```rust
            .field("xiaomimimo_api_key", &redact(&self.xiaomimimo_api_key))
            .field("xiaomimimo_base_url", &self.xiaomimimo_base_url)
            .field("xiaomimimo_rpm", &self.xiaomimimo_rpm)
            .field("copilot_rpm", &self.copilot_rpm)
```

- [ ] **Step 8: Update `partial_to_nested_toml_non_secrets` in config.rs**

Find the function in `crates/scorpio-core/src/config.rs`:
```bash
grep -n "fn partial_to_nested_toml_non_secrets" crates/scorpio-core/src/config.rs
```

Update it to emit only the non-secret nested tables: `[providers.copilot]` when `copilot_rpm` is set, and `[providers.xiaomimimo]` when any of `xiaomimimo_base_url`/`xiaomimimo_rpm` is set. Do not move `xiaomimimo_api_key` into `[providers.xiaomimimo]`; it stays as a top-level secret field on disk.

- [ ] **Step 9: Run the full settings test suite**

```bash
cargo test -p scorpio-core settings::tests::partial_config_round_trips_xiaomimimo_secret_and_copilot_rpm -- --exact
cargo test -p scorpio-core settings::tests::partial_config_debug_redacts_xiaomimimo_secret -- --exact
cargo test -p scorpio-core settings::tests::partial_config_serializes_xiaomimimo_under_providers_table -- --exact
```
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/scorpio-core/src/settings.rs crates/scorpio-core/src/config.rs
git commit -m "$(cat <<'EOF'
feat(settings): persist Xiaomi MiMo secret + Copilot/MiMo non-secret overrides

xiaomimimo_api_key, xiaomimimo_base_url, xiaomimimo_rpm, and copilot_rpm
round-trip through the existing UserConfigFile / UserConfigProviders pipeline.
The Xiaomi MiMo secret stays top-level on disk; only the non-secret base_url/rpm
overrides flow through nested [providers.<name>] tables.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 9: Add a `copilot_token_dir()` helper

**Files:**
- Modify: `crates/scorpio-core/src/settings.rs` (after `user_config_path`)
- Test: same file

- [ ] **Step 1: Find `user_config_path`**

```bash
grep -n "fn user_config_path" crates/scorpio-core/src/settings.rs
```

- [ ] **Step 2: Write the failing test**

Append to settings tests:

```rust
    #[test]
    fn copilot_token_dir_is_under_scorpio_config_root() {
        let dir = copilot_token_dir().expect("token dir resolves");
        assert!(dir.ends_with("github_copilot"),
            "expected suffix github_copilot, got {dir:?}");
        let parent = dir.parent().expect("has parent");
        // Parent must be the scorpio config directory.
        let cfg_path = user_config_path().expect("config path");
        assert_eq!(parent, cfg_path.parent().expect("config has parent"));
    }
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p scorpio-core settings::tests::copilot_token_dir_is_under_scorpio_config_root -- --exact
```
Expected: FAIL — function doesn't exist.

- [ ] **Step 4: Add the helper**

Insert after `user_config_path`:

```rust
/// Resolve the absolute Scorpio-owned Copilot token directory.
///
/// Path: `<config_root>/github_copilot/`. The directory is *not* created here —
/// callers must ensure it exists with `0o700` permissions before passing it to
/// `rig::providers::copilot::Client::builder().token_dir(...)`.
///
/// rig's default token directory (`$XDG_CONFIG_HOME/github_copilot`) is shared
/// with VS Code and JetBrains; deriving the path under Scorpio's config root
/// keeps Scorpio's auth state isolated.
pub fn copilot_token_dir() -> anyhow::Result<PathBuf> {
    let config = user_config_path()?;
    let root = config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("scorpio config path has no parent"))?;
    Ok(root.join("github_copilot"))
}

/// Ensure the Copilot token directory exists with owner-only permissions.
///
/// Creates the directory if missing. On Unix, sets mode `0o700`. Returns the
/// absolute path on success.
pub fn ensure_copilot_token_dir() -> anyhow::Result<PathBuf> {
    let dir = copilot_token_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create copilot token dir at {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(&dir, perms)
            .with_context(|| format!("failed to set 0o700 on {}", dir.display()))?;
    }
    Ok(dir)
}

/// Verify the Copilot token directory is a real, non-symlink directory owned by the
/// current user with mode `0o700` or stricter.
///
/// Returns `Ok(())` on success. Returns an error if the directory is missing,
/// is not a directory, is a symlink, is not owned by the current effective user,
/// or has broader permissions than `0o700`.
#[cfg(unix)]
pub fn verify_copilot_token_dir_secure(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("token directory missing or unreadable: {}", dir.display()))?;
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "copilot token directory at {} must be a real non-symlink directory",
            dir.display()
        ));
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        return Err(anyhow::anyhow!(
            "copilot token directory at {} is not owned by the current user",
            dir.display()
        ));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(anyhow::anyhow!(
            "copilot token directory at {} has insecure permissions {:o} (expected at most 0o700)",
            dir.display(),
            mode
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn verify_copilot_token_dir_secure(_dir: &Path) -> anyhow::Result<()> {
    // Non-Unix platforms: verify only path shape in this slice and rely on the OS
    // ACL model for the stronger confidentiality guarantees.
    Ok(())
}
```

If `libc` isn't a workspace dep, add it to `crates/scorpio-core/Cargo.toml`:
```toml
[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test -p scorpio-core settings::tests::copilot_token_dir_is_under_scorpio_config_root -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/settings.rs crates/scorpio-core/Cargo.toml
git commit -m "feat(settings): add copilot_token_dir + ensure/verify helpers

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 4: Rate Limiting

### Task 10: Add Copilot and Xiaomi MiMo to `ProviderRateLimiters`

**Files:**
- Modify: `crates/scorpio-core/src/rate_limit.rs:136-156, 166-384` (tests)

- [ ] **Step 1: Write the failing tests**

Append to `crates/scorpio-core/src/rate_limit.rs` tests module:

```rust
    #[test]
    fn provider_rate_limiters_construction_includes_copilot() {
        let cfg = providers_config_with(&[(ProviderId::Copilot, 30)]);
        let registry = ProviderRateLimiters::from_config(&cfg);
        assert!(registry.get(ProviderId::Copilot).is_some());
        assert_eq!(
            registry.get(ProviderId::Copilot).map(|l| l.label()),
            Some("copilot")
        );
    }

    #[test]
    fn provider_rate_limiters_construction_includes_xiaomimimo() {
        let cfg = providers_config_with(&[(ProviderId::XiaomiMimo, 50)]);
        let registry = ProviderRateLimiters::from_config(&cfg);
        assert!(registry.get(ProviderId::XiaomiMimo).is_some());
        assert_eq!(
            registry.get(ProviderId::XiaomiMimo).map(|l| l.label()),
            Some("xiaomimimo")
        );
    }
```

Also extend `providers_config_with` and `all_disabled_providers_config` to handle the new variants:

```rust
    fn providers_config_with(overrides: &[(ProviderId, u32)]) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        for &(provider, rpm) in overrides {
            match provider {
                ProviderId::OpenAI => cfg.openai.rpm = rpm,
                ProviderId::Anthropic => cfg.anthropic.rpm = rpm,
                ProviderId::Gemini => cfg.gemini.rpm = rpm,
                ProviderId::OpenRouter => cfg.openrouter.rpm = rpm,
                ProviderId::DeepSeek => cfg.deepseek.rpm = rpm,
                ProviderId::Copilot => cfg.copilot.rpm = rpm,
                ProviderId::XiaomiMimo => cfg.xiaomimimo.rpm = rpm,
            }
        }
        cfg
    }
```

And `all_disabled_providers_config`:

```rust
    fn all_disabled_providers_config() -> ProvidersConfig {
        ProvidersConfig {
            openai: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            anthropic: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            gemini: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            openrouter: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            deepseek: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            copilot: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
            xiaomimimo: ProviderSettings { base_url: None, rpm: 0, ..Default::default() },
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core rate_limit::tests::provider_rate_limiters_construction_includes_copilot -- --exact
```
Expected: FAIL — `Copilot` not in the `provider_rpms` array.

- [ ] **Step 3: Extend `from_config`**

Update `crates/scorpio-core/src/rate_limit.rs:136-142`:

```rust
        let provider_rpms = [
            (ProviderId::OpenAI, cfg.openai.rpm, "openai"),
            (ProviderId::Anthropic, cfg.anthropic.rpm, "anthropic"),
            (ProviderId::Gemini, cfg.gemini.rpm, "gemini"),
            (ProviderId::OpenRouter, cfg.openrouter.rpm, "openrouter"),
            (ProviderId::DeepSeek, cfg.deepseek.rpm, "deepseek"),
            (ProviderId::Copilot, cfg.copilot.rpm, "copilot"),
            (ProviderId::XiaomiMimo, cfg.xiaomimimo.rpm, "xiaomimimo"),
        ];
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p scorpio-core rate_limit::tests::provider_rate_limiters_construction_includes_copilot -- --exact
cargo test -p scorpio-core rate_limit::tests::provider_rate_limiters_construction_includes_xiaomimimo -- --exact
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/rate_limit.rs
git commit -m "feat(rate_limit): include Copilot and Xiaomi MiMo in provider limiters

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 5: Provider Construction (factory/client.rs)

### Task 11: Add `url` workspace dep + Xiaomi MiMo trusted-host validator

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/scorpio-core/Cargo.toml`
- Modify: `crates/scorpio-core/src/providers/factory/client.rs` (validator lives alongside provider construction)

- [ ] **Step 1: Add `url` to workspace dependencies**

Edit `Cargo.toml` `[workspace.dependencies]` block, add:

```toml
url = "2"
```

Edit `crates/scorpio-core/Cargo.toml` `[dependencies]`, add:

```toml
url.workspace = true
```

- [ ] **Step 2: Write the failing test for the URL validator**

Create the test inline in `crates/scorpio-core/src/providers/factory/client.rs` tests module:

```rust
    #[test]
    fn validate_xiaomimimo_base_url_accepts_https() {
        assert!(validate_xiaomimimo_base_url("https://api.xiaomimimo.com/v1").is_ok());
    }

    #[test]
    fn validate_xiaomimimo_base_url_accepts_loopback_http() {
        for url in &["http://127.0.0.1:8080", "http://localhost", "http://[::1]:8080"] {
            assert!(validate_xiaomimimo_base_url(url).is_ok(), "should accept loopback {url}");
        }
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_remote_http() {
        let err = validate_xiaomimimo_base_url("http://api.example.com/v1").unwrap_err();
        assert!(err.to_string().contains("https"), "expected https mention: {err}");
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_localhost_lookalikes() {
        for url in &["http://localhost.evil.com", "https://localhost.evil.com"] {
            assert!(
                validate_xiaomimimo_base_url(url).is_err() || // OK if rejected for protocol
                    {
                        // localhost.evil.com is *not* a loopback host even if it looks like one
                        let parsed = url::Url::parse(url).unwrap();
                        parsed.host_str() != Some("localhost")
                    },
                "must not treat {url} as loopback"
            );
        }
        // Strict assertion for the remote http variant
        assert!(validate_xiaomimimo_base_url("http://localhost.evil.com").is_err());
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_userinfo() {
        let err = validate_xiaomimimo_base_url("https://user@evil.com/").unwrap_err();
        assert!(err.to_string().contains("user"), "expected userinfo mention: {err}");
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_userinfo_with_loopback_lookalike() {
        let err = validate_xiaomimimo_base_url("http://127.0.0.1@evil.com/").unwrap_err();
        assert!(err.to_string().contains("user"), "userinfo: {err}");
    }

    #[test]
    fn validate_xiaomimimo_base_url_rejects_empty() {
        assert!(validate_xiaomimimo_base_url("").is_err());
        assert!(validate_xiaomimimo_base_url("   ").is_err());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::client::tests::validate_xiaomimimo_base_url_accepts_https -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_xiaomimimo_base_url_rejects_remote_http -- --exact
```
Expected: FAIL — function doesn't exist.

- [ ] **Step 4: Implement the validator**

Add to `crates/scorpio-core/src/providers/factory/client.rs` (above the `tests` module):

```rust
/// Validate a Xiaomi MiMo `base_url` per the spec's trusted-host rules.
///
/// - Reject empty/whitespace-only values.
/// - Parse with the `url` crate (never string contains/prefix checks).
/// - Reject any URL with non-empty userinfo.
/// - Reject any URL carrying a query or fragment.
/// - HTTPS scheme: accept only trusted hosts for this slice.
/// - HTTP scheme: accept only when the parsed host is a member of the loopback allowlist
///   (`127.0.0.1`, `::1`, or `localhost`).
/// - Redirects are disabled for custom Xiaomi MiMo overrides; the configured host is
///   the full trust boundary.
fn validate_xiaomimimo_base_url(raw: &str) -> Result<url::Url, TradingError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(config_error("xiaomimimo base_url must not be empty"));
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| config_error(&format!("xiaomimimo base_url is not a valid URL: {e}")))?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(config_error(
            "xiaomimimo base_url must not contain user/password info",
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(config_error(
            "xiaomimimo base_url must not contain query or fragment components",
        ));
    }

    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| config_error("xiaomimimo base_url has no host"))?;

    match scheme {
        "https" => {
            if is_trusted_xiaomimimo_host(host) {
                Ok(parsed)
            } else {
                Err(config_error(&format!(
                    "xiaomimimo base_url host {host:?} is not in the trusted-host allowlist for this slice"
                )))
            }
        }
        "http" => {
            const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "::1", "localhost"];
            if LOOPBACK_HOSTS.contains(&host) {
                Ok(parsed)
            } else {
                Err(config_error(&format!(
                    "xiaomimimo base_url uses http://; only https is allowed except for loopback hosts (got host {host:?})"
                )))
            }
        }
        other => Err(config_error(&format!(
            "xiaomimimo base_url has unsupported scheme {other:?} (expected https or http loopback)"
        ))),
    }
}

fn is_trusted_xiaomimimo_host(host: &str) -> bool {
    matches!(
        host,
        "api.xiaomi.com" | "api.xiaomimimo.com" | "api.mimo.ai"
    )
}
```

Also update setup/docs text in later tasks so operators are told that custom trusted-host overrides send prompts, responses, and the Xiaomi API key to that configured host.

When constructing a Xiaomi MiMo client with a custom `base_url`, pass an explicit `reqwest::Client` (via rig's `.http_client(...)` seam) with redirects disabled so a trusted host cannot bounce requests to a different authority.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p scorpio-core providers::factory::client::tests::validate_xiaomimimo_base_url_accepts_https -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_xiaomimimo_base_url_rejects_remote_http -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/scorpio-core/Cargo.toml crates/scorpio-core/src/providers/factory/client.rs
git commit -m "$(cat <<'EOF'
feat(providers): add xiaomimimo base_url structural validator

Uses the url crate (never string-prefix checks). Allows https://, allows
http:// only for exact loopback hosts {127.0.0.1, ::1, localhost}, and
rejects userinfo-bearing URLs.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 12: Add `CopilotAuthMode` enum and re-export it through the factory facade

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/client.rs` (top of file, public type)
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs` (public re-export)

- [ ] **Step 1: Add the enum**

Insert near the top of `crates/scorpio-core/src/providers/factory/client.rs` (after the `use` block):

```rust
/// Whether a Copilot code path may later trigger interactive OAuth/device-flow auth.
///
/// `rig::providers::copilot::ClientBuilder::build()` only constructs an
/// authenticator; it does not contact GitHub. Scorpio uses this enum to gate
/// which paths may later call `CompletionModelHandle::authorize_copilot()` (or
/// otherwise reach rig's lazy `auth_context()` path). Only `step5_health_check`
/// (the final setup verification step) is allowed to use `InteractiveSetup`;
/// every runtime path uses `NonInteractiveRuntime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotAuthMode {
    /// Setup-time path: may prompt the user with a verification URI and user code.
    InteractiveSetup,
    /// Runtime path: must rely on prevalidated cached auth and never use Scorpio's
    /// interactive setup entrypoint.
    NonInteractiveRuntime,
}

impl Default for CopilotAuthMode {
    fn default() -> Self {
        // Default is conservative: every code path that hasn't explicitly opted into
        // interactive setup gets the runtime gate.
        Self::NonInteractiveRuntime
    }
}
```

- [ ] **Step 2: Re-export it from the factory facade**

Edit `crates/scorpio-core/src/providers/factory/mod.rs` to add `CopilotAuthMode` to the existing client re-export list so `scorpio-cli` can consume it through the public core facade:

```rust
pub use client::{CompletionModelHandle, CopilotAuthMode, create_completion_model};
```

- [ ] **Step 3: Re-export it from the higher-level providers module**

If `crates/scorpio-core/src/providers/mod.rs` already mirrors factory exports, add the matching `pub use factory::CopilotAuthMode;` re-export there too so downstream call sites can follow the existing convention.

- [ ] **Step 4: Build to verify**

```bash
cargo build --workspace
```
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/providers/
git commit -m "feat(providers): add CopilotAuthMode { InteractiveSetup, NonInteractiveRuntime }

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 13: Extend `ProviderClient` enum and `validate_provider_id`

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/client.rs:6, 82-94, 248-259`

- [ ] **Step 1: Write failing tests**

Append to client.rs tests:

```rust
    #[test]
    fn validate_provider_id_accepts_copilot() {
        let result = validate_provider_id("copilot");
        assert_eq!(result.unwrap(), ProviderId::Copilot);
    }

    #[test]
    fn validate_provider_id_accepts_xiaomimimo() {
        let result = validate_provider_id("xiaomimimo");
        assert_eq!(result.unwrap(), ProviderId::XiaomiMimo);
    }

    #[test]
    fn validate_provider_id_unknown_error_lists_new_providers() {
        let err = validate_provider_id("unknown").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("copilot"), "expected copilot in: {msg}");
        assert!(msg.contains("xiaomimimo"), "expected xiaomimimo in: {msg}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_accepts_copilot -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_accepts_xiaomimimo -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_unknown_error_lists_new_providers -- --exact
```
Expected: FAIL.

- [ ] **Step 3: Update the imports and `ProviderClient` variants**

Replace `crates/scorpio-core/src/providers/factory/client.rs:6`:

```rust
use rig::providers::{anthropic, copilot, deepseek, gemini, openai, openrouter, xiaomimimo};
```

Replace lines 82-94:

```rust
#[derive(Debug, Clone)]
pub(crate) enum ProviderClient {
    OpenAI(openai::Client),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
    OpenRouter(openrouter::Client),
    DeepSeek(deepseek::Client),
    Copilot(copilot::Client),
    XiaomiMimo(xiaomimimo::Client),
}
```

- [ ] **Step 4: Update `validate_provider_id`**

Replace lines 248-259:

```rust
fn validate_provider_id(provider: &str) -> Result<ProviderId, TradingError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(ProviderId::OpenAI),
        "anthropic" => Ok(ProviderId::Anthropic),
        "gemini" => Ok(ProviderId::Gemini),
        "openrouter" => Ok(ProviderId::OpenRouter),
        "deepseek" => Ok(ProviderId::DeepSeek),
        "copilot" => Ok(ProviderId::Copilot),
        "xiaomimimo" => Ok(ProviderId::XiaomiMimo),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek, copilot, xiaomimimo)"
        ))),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_accepts_copilot -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_accepts_xiaomimimo -- --exact
cargo test -p scorpio-core providers::factory::client::tests::validate_provider_id_unknown_error_lists_new_providers -- --exact
```
Expected: PASS. Some other tests may still fail because `create_provider_client_for` is non-exhaustive — that's addressed in the next task.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/client.rs
git commit -m "feat(providers): add Copilot and XiaomiMimo to ProviderClient enum

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 14: Add Xiaomi MiMo client construction branch and keep Copilot routed through the shared factory seam

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/client.rs` (in `create_provider_client_for`)

- [ ] **Step 1: Write failing tests**

Append to client.rs tests:

```rust
    fn providers_config_with_xiaomimimo() -> ProvidersConfig {
        ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(SecretString::from("mimo-test-key")),
                base_url: None,
                rpm: 50,
            },
            ..Default::default()
        }
    }

    #[test]
    fn factory_creates_xiaomimimo_client() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers_config_with_xiaomimimo(),
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "xiaomimimo");
        assert!(matches!(handle.client, ProviderClient::XiaomiMimo(_)));
    }

    #[test]
    fn factory_missing_xiaomimimo_key_returns_config_error() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let result = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        );
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("SCORPIO_XIAOMIMIMO_API_KEY"), "got: {msg}");
    }

    #[test]
    fn factory_xiaomimimo_with_https_base_url_succeeds() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let providers = ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(SecretString::from("mimo-test-key")),
                base_url: Some("https://api.xiaomimimo.com/v1".to_owned()),
                rpm: 50,
            },
            ..Default::default()
        };
        let handle = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
        )
        .unwrap();
        assert!(matches!(handle.client, ProviderClient::XiaomiMimo(_)));
    }

    #[test]
    fn factory_xiaomimimo_with_http_remote_base_url_rejected() {
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "xiaomimimo".to_owned();
        cfg.quick_thinking_model = "mimo-v2.5".to_owned();
        let providers = ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(SecretString::from("mimo-test-key")),
                base_url: Some("http://api.example.com/v1".to_owned()),
                rpm: 50,
            },
            ..Default::default()
        };
        let err = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("https"), "got: {err}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::client::tests::factory_creates_xiaomimimo_client -- --exact
```
Expected: FAIL — non-exhaustive `match` in `create_provider_client_for`.

- [ ] **Step 3: Add the Xiaomi MiMo branch**

In `create_provider_client_for` at the bottom of the existing `match provider { ... }`, add:

```rust
        ProviderId::XiaomiMimo => {
            let key = settings
                .api_key
                .as_ref()
                .ok_or_else(|| missing_key_error(provider))?;
            let client = match settings.base_url.as_deref() {
                Some(raw_url) => {
                    let parsed = validate_xiaomimimo_base_url(raw_url)?;
                    xiaomimimo::Client::builder()
                        .api_key(key.expose_secret())
                        .base_url(parsed.to_string())
                        .build()
                        .map_err(|e| {
                            config_error(&format!(
                                "failed to create Xiaomi MiMo client with base_url \"{raw_url}\": {e}"
                            ))
                        })?
                }
                None => xiaomimimo::Client::new(key.expose_secret())
                    .map_err(|e| config_error(&format!("failed to create Xiaomi MiMo client: {e}")))?,
            };
            Ok(ProviderClient::XiaomiMimo(client))
        }
```

- [ ] **Step 4: Add a temporary Copilot branch that delegates to the upcoming auth-mode helper**

Do **not** make bare `create_completion_model(...)` reject Copilot. The design spec requires `create_completion_model(...)` to remain the runtime seam. For this task, make the arm compile by delegating to a private `create_copilot_client_for(...)` helper with the exact call shape below:

```rust
        ProviderId::Copilot => create_copilot_client_for(
            provider,
            settings,
            CopilotAuthMode::NonInteractiveRuntime,
        ),
```

In this task, the helper may temporarily return a `TradingError::Config("copilot auth-mode branch not implemented yet")` placeholder. Task 15 replaces that placeholder with the full implementation.

- [ ] **Step 5: Run tests to verify Xiaomi MiMo tests pass**

```bash
cargo test -p scorpio-core providers::factory::client::tests::factory_creates_xiaomimimo_client -- --exact
cargo test -p scorpio-core providers::factory::client::tests::factory_missing_xiaomimimo_key_returns_config_error -- --exact
cargo test -p scorpio-core providers::factory::client::tests::factory_xiaomimimo_with_https_base_url_succeeds -- --exact
cargo test -p scorpio-core providers::factory::client::tests::factory_xiaomimimo_with_http_remote_base_url_rejected -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/client.rs
git commit -m "feat(providers): add Xiaomi MiMo client construction with URL validation

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 15: Add Copilot client construction with `CopilotAuthMode` plus an explicit auth trigger while keeping `create_completion_model(...)` as the runtime entrypoint

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/client.rs`
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs` (re-export `create_completion_model_with_copilot`)

- [ ] **Step 1: Write failing tests**

Append to client.rs tests:

```rust
    #[test]
    fn factory_creates_copilot_client_in_interactive_setup_mode() {
        // Use a tempdir for the token directory — the test should not pollute the
        // user's real config directory.
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let providers = ProvidersConfig {
            copilot: ProviderSettings { api_key: None, base_url: None, rpm: 30 },
            ..Default::default()
        };

        let handle = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &providers,
            &ProviderRateLimiters::default(),
            CopilotAuthMode::InteractiveSetup,
            &token_dir,
        )
        .unwrap();
        assert_eq!(handle.provider_name(), "copilot");
        assert!(matches!(handle.client, ProviderClient::Copilot(_)));
    }

    #[test]
    fn factory_runtime_mode_fails_when_token_cache_missing() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        // Note: no token files written.

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("scorpio setup"), "expected setup guidance: {msg}");
    }

    #[test]
    fn factory_default_create_completion_model_uses_noninteractive_copilot_runtime() {
        // The default runtime seam must continue to be create_completion_model.
        // It should route Copilot through NonInteractiveRuntime automatically and
        // fail with setup guidance when the token cache is missing.
        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();
        let err = create_completion_model(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("scorpio setup"), "got: {err}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::client::tests::factory_creates_copilot_client_in_interactive_setup_mode -- --exact
```
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Add a shared Copilot helper, re-export the setup-only entry point, and expose an explicit auth trigger**

Keep `create_completion_model(...)` as the public runtime seam. Add a shared private helper like `create_copilot_client_for(mode, token_dir_override, provider, settings, ...)` that both runtime and setup can call. Then add a small public setup-only helper alongside `create_completion_model(...)`:

```rust
/// Construct a completion-model handle for Copilot with an explicit auth mode.
///
/// `token_dir` must be an absolute, owner-only directory dedicated to Scorpio's
/// Copilot auth state (see `crate::settings::ensure_copilot_token_dir`).
///
/// In `InteractiveSetup` mode this function only builds the handle; callers must
/// explicitly call `CompletionModelHandle::authorize_copilot()` to trigger rig's
/// lazy auth path. The interactive builder path must also install an explicit
/// `on_device_code` callback that prints the verification URI and user code to
/// the terminal so Scorpio owns the operator UX rather than relying on rig's
/// default stdout behavior. In `NonInteractiveRuntime` mode this function refuses
/// to construct the client when the token cache directory does not contain the
/// expected files, and installs a no-op device-code handler so Scorpio never
/// intentionally surfaces an interactive prompt from runtime code. This is a
/// runtime guardrail, not a claim that rig internals can never attempt a lazy
/// auth path on their own.
pub fn create_completion_model_with_copilot(
    tier: ModelTier,
    llm_config: &LlmConfig,
    providers_config: &ProvidersConfig,
    rate_limiters: &ProviderRateLimiters,
    mode: CopilotAuthMode,
    token_dir: &std::path::Path,
) -> Result<CompletionModelHandle, TradingError> {
    // Setup-only helper: callers pass the tier that is actually routed to Copilot.
    let provider = validate_provider_id(tier.provider_id(llm_config))?;
    let model_id = validate_model_id(tier.model_id(llm_config))?;
    if provider != ProviderId::Copilot {
        return create_completion_model(tier, llm_config, providers_config, rate_limiters);
    }

    if mode == CopilotAuthMode::NonInteractiveRuntime {
        // Pre-flight check: token cache must exist.
        let access_token_file = token_dir.join("access-token");
        let api_key_file = token_dir.join("api-key.json");
        if !access_token_file.is_file() || !api_key_file.is_file() {
            return Err(config_error(
                "Copilot token cache is missing under the Scorpio config dir; \
                 run `scorpio setup` to authorize Copilot",
            ));
        }
    }

    let mut builder = copilot::Client::builder()
        .oauth()
        .token_dir(token_dir);

    if mode == CopilotAuthMode::NonInteractiveRuntime {
        // No-op device-code handler — runtime code must not intentionally surface
        // interactive auth, even if rig later falls through to its lazy auth path.
        builder = builder.on_device_code(|_prompt| {
            tracing::error!(
                "Copilot device flow attempted in non-interactive runtime mode; refusing to prompt"
            );
        });
    }

    let client = builder.build().map_err(|e| {
        config_error(&format!("failed to construct Copilot client: {e}"))
    })?;

    let rate_limiter = rate_limiters.get(provider).cloned();
    info!(
        provider = provider.as_str(),
        model = model_id.as_str(),
        tier = %tier,
        mode = ?mode,
        "Copilot completion model handle created"
    );
    Ok(CompletionModelHandle {
        provider,
        model_id,
        client: ProviderClient::Copilot(client),
        rate_limiter,
    })
}

impl CompletionModelHandle {
    /// Trigger rig's lazy Copilot authorization path for setup-time flows.
    ///
    /// This is the exact operation that may reuse cached auth or open GitHub's
    /// device flow on cache miss. `copilot::Client::builder().build()` does not
    /// contact GitHub by itself.
    pub async fn authorize_copilot(&self) -> Result<(), TradingError> {
        match &self.client {
            ProviderClient::Copilot(client) => client
                .authorize()
                .await
                .map_err(|e| config_error(&format!("failed to authorize Copilot client: {e}"))),
            _ => Err(config_error(
                "authorize_copilot requires a Copilot completion model handle",
            )),
        }
    }
}
```

Also update `crates/scorpio-core/src/providers/factory/mod.rs` to re-export `create_completion_model_with_copilot` alongside `create_completion_model` so `scorpio-cli::cli::setup::steps` can call it through the public core facade.

Then update the existing `create_completion_model(...)` implementation so its `ProviderId::Copilot` arm automatically resolves Scorpio's managed token directory and calls the same shared helper in `CopilotAuthMode::NonInteractiveRuntime`. This is what keeps Copilot first-class without migrating every runtime caller.

Use the `rig-core 0.36.0` Copilot builder/auth APIs exactly as shown here: `.oauth()`, `.token_dir(...)`, `.on_device_code(...)`, and `client.authorize().await`. If the local dependency version does not expose that API, stop and update the plan/spec before implementation rather than improvising a different auth seam.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p scorpio-core providers::factory::client::tests::factory_creates_copilot_client_in_interactive_setup_mode -- --exact
cargo test -p scorpio-core providers::factory::client::tests::factory_runtime_mode_fails_when_token_cache_missing -- --exact
cargo test -p scorpio-core providers::factory::client::tests::factory_default_create_completion_model_uses_noninteractive_copilot_runtime -- --exact
```
Expected: PASS.

- [ ] **Step 5: Add an enforceable regression test forbidding `from_env`**

Add a regression test in `crates/scorpio-core/src/providers/factory/client.rs` (or the nearest provider-factory test module) that asserts Scorpio's Copilot construction path never uses `copilot::Client::from_env()` and therefore cannot bypass Scorpio's token-dir flow.

```rust
    #[test]
    fn copilot_factory_paths_do_not_use_from_env() {
        let source = include_str!("client.rs");
        assert!(
            !source.contains("copilot::Client::from_env"),
            "Copilot factory must not use from_env; it bypasses Scorpio-managed token_dir auth"
        );
    }
```

Keep the explanatory SECURITY comment if it helps readers, but the test is the enforcement mechanism.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/client.rs
git commit -m "$(cat <<'EOF'
feat(providers): construct Copilot clients via CopilotAuthMode-gated factory paths

InteractiveSetup is the only Scorpio path allowed to call
CompletionModelHandle::authorize_copilot(), which triggers rig's lazy
OAuth/device-flow path on cache miss. NonInteractiveRuntime remains the
default runtime path behind create_completion_model(...), pre-flight-checks
the token cache, and installs a no-op device-code handler so Scorpio runtime
code does not intentionally surface interactive auth.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: Agent Construction (factory/agent.rs)

### Task 16: Add Copilot and Xiaomi MiMo type aliases and dispatch arms

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/agent.rs:38-53` (type aliases + dispatch macro), `LlmAgentInner` enum, `build_agent_inner`

- [ ] **Step 1: Read the existing dispatch macro and `LlmAgentInner` enum**

```bash
sed -n '38,75p' crates/scorpio-core/src/providers/factory/agent.rs
sed -n '650,740p' crates/scorpio-core/src/providers/factory/agent.rs
```

- [ ] **Step 2: Write the failing test**

Append to agent.rs tests:

```rust
    #[test]
    fn build_agent_supports_copilot_variant() {
        // Use a real Copilot client via the test path. We don't actually call
        // the model; we just verify the agent enum dispatches correctly.
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        let client = rig::providers::copilot::Client::builder()
            .oauth()
            .token_dir(&token_dir)
            .build()
            .expect("copilot client construction");
        let handle = CompletionModelHandle::for_test_with_client(
            ProviderId::Copilot,
            "gpt-4o",
            ProviderClient::Copilot(client),
        );
        let agent = build_agent(&handle, "test prompt");
        assert!(matches!(&agent.inner, LlmAgentInner::Copilot(_)));
    }

    #[test]
    fn build_agent_supports_xiaomimimo_variant() {
        let client = rig::providers::xiaomimimo::Client::new("test-key")
            .expect("client construction");
        let handle = CompletionModelHandle::for_test_with_client(
            ProviderId::XiaomiMimo,
            "mimo-v2.5",
            ProviderClient::XiaomiMimo(client),
        );
        let agent = build_agent(&handle, "test prompt");
        assert!(matches!(&agent.inner, LlmAgentInner::XiaomiMimo(_)));
    }
```

Add `CompletionModelHandle::for_test_with_client(provider, model_id, client)` to `crates/scorpio-core/src/providers/factory/client.rs` as a `#[cfg(any(test, feature = "test-helpers"))]` helper so the new agent tests can build concrete Copilot and Xiaomi MiMo handles without calling private fields.

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::agent::tests::build_agent_supports_copilot_variant -- --exact
```
Expected: FAIL — `LlmAgentInner::Copilot` doesn't exist.

- [ ] **Step 4: Add type aliases**

Update `crates/scorpio-core/src/providers/factory/agent.rs:38-43`:

```rust
type OpenAIModel = rig::providers::openai::responses_api::ResponsesCompletionModel;
type AnthropicModel = rig::providers::anthropic::completion::CompletionModel;
type GeminiModel = rig::providers::gemini::completion::CompletionModel;
type OpenRouterModel = rig::providers::openrouter::completion::CompletionModel;
type DeepSeekModel = rig::providers::deepseek::CompletionModel;
type CopilotModel = rig::providers::copilot::CompletionModel<reqwest::Client>;
type XiaomiMimoModel = rig::providers::openai::completion::GenericCompletionModel<
    rig::providers::xiaomimimo::XiaomiMimoExt,
    reqwest::Client,
>;
```

(If the actual rig path for `XiaomiMimoExt` differs — e.g., `rig::providers::xiaomimimo::XiaomiMimoExt` is exposed at a different module — adjust the path. To find it:

```bash
cargo doc --no-deps -p rig-core 2>&1 | grep -i "xiaomimimo"
# or, if the rig source is in ~/.cargo:
grep -rn "pub.*XiaomiMimoExt\|impl.*XiaomiMimoExt" ~/.cargo/registry/src/*/rig-core-0.36.0/src/providers/xiaomimimo* 2>/dev/null | head
```)

- [ ] **Step 5: Update the `dispatch_llm_agent!` macro**

Replace lines 44-55:

```rust
macro_rules! dispatch_llm_agent {
    ($self:ident, |$agent:ident| $body:expr, |$mock:ident| $mock_body:expr $(,)?) => {
        match &$self.inner {
            LlmAgentInner::OpenAI($agent) => $body,
            LlmAgentInner::Anthropic($agent) => $body,
            LlmAgentInner::Gemini($agent) => $body,
            LlmAgentInner::OpenRouter($agent) => $body,
            LlmAgentInner::DeepSeek($agent) => $body,
            LlmAgentInner::Copilot($agent) => $body,
            LlmAgentInner::XiaomiMimo($agent) => $body,
            #[cfg(any(test, feature = "test-helpers"))]
            LlmAgentInner::Mock($mock) => $mock_body,
        }
    };
}
```

- [ ] **Step 6: Update `LlmAgentInner` enum**

Find the enum (~line 67) and add:

```rust
enum LlmAgentInner {
    OpenAI(rig::agent::Agent<OpenAIModel>),
    Anthropic(rig::agent::Agent<AnthropicModel>),
    Gemini(rig::agent::Agent<GeminiModel>),
    OpenRouter(rig::agent::Agent<OpenRouterModel>),
    DeepSeek(rig::agent::Agent<DeepSeekModel>),
    Copilot(rig::agent::Agent<CopilotModel>),
    XiaomiMimo(rig::agent::Agent<XiaomiMimoModel>),
    #[cfg(any(test, feature = "test-helpers"))]
    Mock(/* keep existing variant body */),
}
```

Match the exact `rig::agent::Agent<...>` path and generic structure already used by the existing OpenAI/Anthropic/Gemini/OpenRouter/DeepSeek variants in `crates/scorpio-core/src/providers/factory/agent.rs`; do not introduce a parallel alias style just for the new providers.

- [ ] **Step 7: Update `build_agent_inner` to handle the new providers**

Find `build_agent_inner` in `crates/scorpio-core/src/providers/factory/agent.rs` and extend its existing `match` on `ProviderClient` with:

```rust
        ProviderClient::Copilot(client) => {
            let builder = build_completion_agent_builder(client, &handle.model_id);
            let agent = match tools {
                Some(tools) => attach_tools(builder, tools).build(),
                None => builder.build(),
            };
            LlmAgent {
                model_id: handle.model_id.clone(),
                inner: LlmAgentInner::Copilot(agent),
            }
        }
        ProviderClient::XiaomiMimo(client) => {
            let builder = build_completion_agent_builder(client, &handle.model_id);
            let agent = match tools {
                Some(tools) => attach_tools(builder, tools).build(),
                None => builder.build(),
            };
            LlmAgent {
                model_id: handle.model_id.clone(),
                inner: LlmAgentInner::XiaomiMimo(agent),
            }
        }
```

(Whatever helper exists for OpenAI/etc. should be re-used; the snippet uses an illustrative structure — match the actual code in `build_agent_inner`.)

If Anthropic uses `.max_tokens(4096)`, decide whether Copilot/MiMo need the same. **For this slice, do not add `.max_tokens`** unless the model requires it; record the decision in a comment.

- [ ] **Step 8: Run tests**

```bash
cargo test -p scorpio-core providers::factory::agent::tests::build_agent_supports_copilot_variant -- --exact
cargo test -p scorpio-core providers::factory::agent::tests::build_agent_supports_xiaomimimo_variant -- --exact
```
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/agent.rs crates/scorpio-core/src/providers/factory/client.rs
git commit -m "$(cat <<'EOF'
feat(providers): wire Copilot and XiaomiMimo through agent factory

Type aliases use rig's CompletionModel<reqwest::Client> for Copilot and the
GenericCompletionModel<XiaomiMimoExt, reqwest::Client> alias for Xiaomi MiMo.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 17: Token usage extraction for Copilot Chat/Responses

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/agent.rs` (wherever token usage is extracted from response)

- [ ] **Step 1: Find the existing usage extraction**

```bash
grep -n "TokenUsage\|prompt_tokens\|completion_tokens\|total_tokens" crates/scorpio-core/src/providers/factory/agent.rs crates/scorpio-core/src/state/ | head -20
```

- [ ] **Step 2: Verify whether Scorpio's existing shared usage seam already handles Copilot**

The Copilot client returns `CopilotCompletionResponse::Chat(ChatCompletionResponse)` (which has `usage.prompt_tokens, usage.completion_tokens, usage.total_tokens`) or `CopilotCompletionResponse::Responses(Box<ResponsesCompletionResponse>)` (which has `usage.input_tokens, usage.output_tokens, usage.total_tokens`).

- [ ] **Step 3: Only add provider-specific helpers if the shared usage seam is insufficient**

First, add a characterization test against the current usage-extraction path. If that passes for both Copilot response shapes, stop here and record that no implementation change is needed. If it fails, then add a narrow helper in `agent.rs` and route Copilot through it.

If a dedicated fix is needed, the response-handling code can add a `match` arm like:

```rust
        ProviderClient::Copilot(_) => {
            // The wrapping CopilotCompletionResponse is normalized by rig before
            // reaching this point — extract via the public Usage trait if
            // present, otherwise pattern-match on the response variant.
            // <existing rig::completion::Usage trait extraction>
        }
```

Concretely, if `rig::completion::CompletionResponse` exposes a `usage()` method (Trait), simply call it; the trait is uniform across providers. If not, add a per-variant extraction.

- [ ] **Step 4: Add a unit test that exercises both Copilot response variants directly**

The `rig::completion::Usage` trait (or the equivalent type rig 0.36 exposes for both variants) is the seam here. Build a `Usage` value of each shape and pass it to `extract_token_usage` (or whatever Scorpio's helper is named). Concrete test:

```rust
    #[test]
    fn copilot_chat_usage_extracts_total_tokens() {
        // Build a Usage matching openai::completion::Usage shape.
        let usage = rig::providers::openai::completion::Usage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        };
        let record = token_usage_from_openai_usage(&usage);
        assert_eq!(record.input_tokens, 10);
        assert_eq!(record.output_tokens, 20);
        assert_eq!(record.total_tokens, 30);
    }

    #[test]
    fn copilot_responses_usage_extracts_total_tokens() {
        // Build a ResponsesUsage matching the Responses API shape.
        let usage = rig::providers::openai::responses_api::ResponsesUsage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
        };
        let record = token_usage_from_responses_usage(&usage);
        assert_eq!(record.input_tokens, 10);
        assert_eq!(record.output_tokens, 20);
        assert_eq!(record.total_tokens, 30);
    }
```

If the exact rig field names differ (e.g., `prompt_tokens` vs `input_tokens`), match them to the rig 0.36.0 source. If the helpers `token_usage_from_openai_usage` / `token_usage_from_responses_usage` don't yet exist, add them as small private functions in `agent.rs` and have the response-handling code call them.

- [ ] **Step 5: Run agent tests**

```bash
cargo test -p scorpio-core providers::factory::agent::tests::copilot_chat_usage_extracts_total_tokens -- --exact
cargo test -p scorpio-core providers::factory::agent::tests::copilot_responses_usage_extracts_total_tokens -- --exact
```
Expected: existing tests pass.

- [ ] **Step 6: Commit (only if Step 3 required an implementation change)**

```bash
git add crates/scorpio-core/src/providers/factory/agent.rs
git commit -m "feat(providers): handle Copilot Chat vs Responses token usage shapes

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 7: Discovery

### Task 18: Add Copilot static curated list and Xiaomi MiMo discovery

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/discovery.rs`
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs` (re-export setup-facing discovery constant/accessor)

- [ ] **Step 1: Write the failing test**

Append to discovery.rs tests:

```rust
    #[tokio::test]
    async fn copilot_returns_curated_static_list_without_network() {
        let providers = ProvidersConfig::default(); // No copilot key, no client construction.
        let outcomes = discover_setup_models(&[ProviderId::Copilot], &providers).await;
        let outcome = outcomes.get(&ProviderId::Copilot).expect("copilot present");
        let ModelDiscoveryOutcome::Listed(models) = outcome else {
            panic!("expected Listed, got {outcome:?}");
        };
        // The curated list should include common GPT models and exclude Codex models for slice 1.
        assert!(models.contains(&"gpt-4o".to_owned()));
        assert!(models.contains(&"gpt-4o-mini".to_owned()));
        assert!(models.contains(&"claude-sonnet-4".to_owned()));
        assert!(
            !models.iter().any(|m| m.contains("codex")),
            "no Codex models in slice 1: {models:?}"
        );
    }

    #[tokio::test]
    async fn xiaomimimo_with_base_url_returns_unavailable() {
        let providers = ProvidersConfig {
            xiaomimimo: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("test-key")),
                base_url: Some("https://api.xiaomimimo.com/v1".to_owned()),
                rpm: 50,
            },
            ..Default::default()
        };
        let outcomes = discover_setup_models(&[ProviderId::XiaomiMimo], &providers).await;
        let outcome = outcomes.get(&ProviderId::XiaomiMimo).expect("present");
        assert!(matches!(outcome, ModelDiscoveryOutcome::Unavailable { .. }));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::discovery::tests::copilot_returns_curated_static_list_without_network -- --exact
cargo test -p scorpio-core providers::factory::discovery::tests::xiaomimimo_with_base_url_returns_unavailable -- --exact
```
Expected: FAIL — `ProviderId::Copilot` not handled.

- [ ] **Step 3: Add the Copilot curated list constant**

Insert near the top of `crates/scorpio-core/src/providers/factory/discovery.rs`:

```rust
/// Curated Copilot model list for setup picker (slice 1).
///
/// Codex-class models are deliberately omitted because rig routes any model whose
/// lowercase name contains "codex" to the Responses API endpoint, which uses a
/// different request/response shape and may not interact correctly with Scorpio's
/// structured-output and tool-calling paths.
pub const COPILOT_CURATED_MODELS: &[&str] = &[
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-4.1",
    "claude-sonnet-4",
    "gemini-2.0-flash-001",
    "o3-mini",
];
```

Then re-export this from `crates/scorpio-core/src/providers/factory/mod.rs` as a public setup-facing constant or accessor, e.g. `pub use discovery::COPILOT_CURATED_MODELS;`, so `scorpio-cli` can use the same source of truth without reaching into a private module.

- [ ] **Step 4: Update the `match provider` arm in `discover_setup_models`**

Replace lines 36-46:

```rust
pub async fn discover_setup_models(
    eligible: &[ProviderId],
    providers: &ProvidersConfig,
) -> HashMap<ProviderId, ModelDiscoveryOutcome> {
    // Pre-compute Copilot outcome statically — never reach the load() closure.
    let mut outcomes: HashMap<ProviderId, ModelDiscoveryOutcome> = eligible
        .iter()
        .copied()
        .filter(|p| *p == ProviderId::Copilot)
        .map(|p| {
            (
                p,
                ModelDiscoveryOutcome::Listed(
                    COPILOT_CURATED_MODELS.iter().map(|s| s.to_string()).collect(),
                ),
            )
        })
        .collect();

    let dynamic: Vec<ProviderId> = eligible
        .iter()
        .copied()
        .filter(|p| *p != ProviderId::Copilot)
        .collect();

    let dynamic_outcomes = discover_setup_models_with(dynamic, |provider| async move {
        match provider {
            ProviderId::OpenRouter => Err("manual-only".to_owned()),
            ProviderId::OpenAI => list_openai_models(&providers.openai).await,
            ProviderId::Anthropic => list_anthropic_models(&providers.anthropic).await,
            ProviderId::Gemini => list_gemini_models(&providers.gemini).await,
            ProviderId::DeepSeek => list_deepseek_models(&providers.deepseek).await,
            ProviderId::XiaomiMimo => list_xiaomimimo_models(&providers.xiaomimimo).await,
            ProviderId::Copilot => unreachable!(
                "Copilot is short-circuited above; never reaches the load closure"
            ),
        }
    })
    .await;

    outcomes.extend(dynamic_outcomes);
    outcomes
}
```

- [ ] **Step 5: Add `list_xiaomimimo_models`**

Append after `list_deepseek_models`:

```rust
async fn list_xiaomimimo_models(settings: &ProviderSettings) -> Result<ModelList, String> {
    if settings.base_url.is_some() {
        return Err("custom base_url requires manual entry".to_owned());
    }
    let key = settings
        .api_key
        .as_ref()
        .ok_or_else(|| "missing API key".to_owned())?;
    let client = rig::providers::xiaomimimo::Client::new(key.expose_secret())
        .map_err(|e| format!("client build error: {e}"))?;
    let raw = client.list_models().await.map_err(|e| e.to_string())?;
    Ok(sanitize_xiaomimimo_model_ids(raw))
}

/// Filter and escape provider-supplied model IDs so control characters and
/// pathological strings cannot reach the terminal or the saved config.
fn sanitize_xiaomimimo_model_ids(list: ModelList) -> ModelList {
    use rig::model::Model;
    let safe: Vec<Model> = list
        .into_iter()
        .filter(|m| is_safe_model_id(&m.id))
        .collect();
    ModelList::new(safe)
}

fn is_safe_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.chars().any(|c| c.is_control() || c == '\u{7f}')
}
```

- [ ] **Step 6: Add a sanitization test**

```rust
    #[test]
    fn sanitize_xiaomimimo_model_ids_drops_control_chars() {
        use rig::model::{Model, ModelList};
        let raw = ModelList::new(vec![
            Model::from_id("good-model"),
            Model::from_id("bad\nmodel"),
            Model::from_id("\x07ringmodel"),
            Model::from_id(""),
        ]);
        let sanitized = sanitize_xiaomimimo_model_ids(raw);
        let ids: Vec<String> = sanitized.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["good-model".to_owned()]);
    }
```

- [ ] **Step 7: Run tests to verify they pass**

```bash
cargo test -p scorpio-core providers::factory::discovery::tests::copilot_returns_curated_static_list_without_network -- --exact
cargo test -p scorpio-core providers::factory::discovery::tests::xiaomimimo_with_base_url_returns_unavailable -- --exact
cargo test -p scorpio-core providers::factory::discovery::tests::sanitize_xiaomimimo_model_ids_drops_control_chars -- --exact
```
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/discovery.rs
git commit -m "$(cat <<'EOF'
feat(discovery): add Copilot curated list + Xiaomi MiMo listing

Copilot uses a static curated list (no network call, no client construction)
and is short-circuited before the load closure since CopilotExt does not
implement ModelListingClient. Xiaomi MiMo uses rig's list_models() and runs
returned IDs through a sanitizer that drops control characters and oversized
strings before they can reach the terminal or config file.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8: Setup Wizard

### Task 19: Split keyed-provider prompting from routing eligibility + make routing eligibility use the effective merged provider config

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs:21-28, 229-258`

- [ ] **Step 1: Read the current state**

```bash
sed -n '15,30p' crates/scorpio-cli/src/cli/setup/steps.rs
sed -n '225,260p' crates/scorpio-cli/src/cli/setup/steps.rs
```

- [ ] **Step 2: Write the failing tests**

Append to steps.rs tests:

```rust
    #[test]
    fn keyed_wizard_providers_excludes_copilot() {
        assert!(!KEYED_WIZARD_PROVIDERS.contains(&ProviderId::Copilot));
        assert!(KEYED_WIZARD_PROVIDERS.contains(&ProviderId::OpenAI));
        assert!(KEYED_WIZARD_PROVIDERS.contains(&ProviderId::XiaomiMimo));
    }

    #[test]
    fn routing_eligible_providers_includes_copilot_when_no_keys() {
        let partial = PartialConfig::default();
        let eligible = eligible_routing_providers(&partial, &ProvidersConfig::default());
        assert_eq!(eligible, vec![ProviderId::Copilot]);
    }

    #[test]
    fn routing_eligible_providers_appends_copilot_after_effective_keyed_providers() {
        let partial = PartialConfig::default();
        let providers = ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("sk-test")),
                ..Default::default()
            },
            ..Default::default()
        };
        let eligible = eligible_routing_providers(&partial, &providers);
        assert_eq!(eligible, vec![ProviderId::OpenAI, ProviderId::Copilot]);
    }

    #[test]
    fn validate_step3_result_passes_with_copilot_only_flag() {
        let partial = PartialConfig::default();
        // Without flag: errs.
        assert!(validate_step3_result(&partial, &ProvidersConfig::default(), false).is_err());
        // With flag: ok.
        assert!(validate_step3_result(&partial, &ProvidersConfig::default(), true).is_ok());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p scorpio-cli cli::setup::steps::tests::keyed_wizard_providers_excludes_copilot -- --exact
```
Expected: FAIL — symbols don't exist.

- [ ] **Step 4: Replace `WIZARD_PROVIDERS` with `KEYED_WIZARD_PROVIDERS` and add `eligible_routing_providers`**

Update `crates/scorpio-cli/src/cli/setup/steps.rs:21-28`:

```rust
/// Step-3 keyed providers — those for which the wizard prompts for an API key.
/// Copilot is intentionally excluded (it uses OAuth, not an API key).
pub const KEYED_WIZARD_PROVIDERS: &[ProviderId] = &[
    ProviderId::OpenAI,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenRouter,
    ProviderId::DeepSeek,
    ProviderId::XiaomiMimo,
];
```

If `WIZARD_PROVIDERS` is referenced elsewhere in the CLI, replace usage with `KEYED_WIZARD_PROVIDERS` (or keep `WIZARD_PROVIDERS` as a deprecated alias and migrate callers in this same task).

- [ ] **Step 4a: Add a partial-aware provider-only config loader before any CLI call sites use it**

In `crates/scorpio-core/src/config.rs`, add or update `Config::load_effective_providers_config_from_user_path(...)` so it accepts both the on-disk config path and an in-memory `&PartialConfig`, merges those sources plus env overrides, and returns `ProvidersConfig` without requiring valid `[llm]` routing. This is a prerequisite for the Step-3/Step-4 CLI call sites in Tasks 20-21.

- [ ] **Step 5: Update `validate_step3_result` signature and body**

Replace lines 229-242:

```rust
pub(super) fn validate_step3_result(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
    copilot_only_selected: bool,
) -> Result<(), &'static str> {
    if copilot_only_selected {
        return Ok(());
    }
    if providers_with_keys(partial, effective_providers).is_empty() {
        Err("At least one LLM provider is required (or pick the Copilot-only path)")
    } else {
        Ok(())
    }
}
```

- [ ] **Step 6: Update `providers_with_keys` to include Xiaomi MiMo**

Replace lines 244-249:

```rust
pub(super) fn providers_with_keys(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
) -> Vec<ProviderId> {
    KEYED_WIZARD_PROVIDERS
        .iter()
        .filter(|p| match **p {
            ProviderId::OpenAI => effective_providers.openai.api_key.is_some() || partial.openai_api_key.is_some(),
            ProviderId::Anthropic => effective_providers.anthropic.api_key.is_some() || partial.anthropic_api_key.is_some(),
            ProviderId::Gemini => effective_providers.gemini.api_key.is_some() || partial.gemini_api_key.is_some(),
            ProviderId::OpenRouter => effective_providers.openrouter.api_key.is_some() || partial.openrouter_api_key.is_some(),
            ProviderId::DeepSeek => effective_providers.deepseek.api_key.is_some() || partial.deepseek_api_key.is_some(),
            ProviderId::XiaomiMimo => effective_providers.xiaomimimo.api_key.is_some() || partial.xiaomimimo_api_key.is_some(),
            // Copilot has no key — not in KEYED_WIZARD_PROVIDERS, but the match
            // must remain exhaustive on changes to ProviderId.
            ProviderId::Copilot => false,
        })
        .copied()
        .collect()
}
```

- [ ] **Step 7: Add `eligible_routing_providers`**

Insert near `providers_with_keys`:

```rust
/// Step-4 routing eligibility: keyed providers with secrets, plus Copilot.
///
/// Copilot is always appended at the end so existing default-selection behavior
/// stays stable and Copilot does not become the implicit first choice.
pub(super) fn eligible_routing_providers(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
) -> Vec<ProviderId> {
    let mut eligible = providers_with_keys(partial, effective_providers);
    eligible.push(ProviderId::Copilot);
    eligible
}
```

- [ ] **Step 8: Update all call sites that previously used `WIZARD_PROVIDERS` or the old `validate_step3_result`/`providers_with_keys` signatures**

```bash
grep -rn "WIZARD_PROVIDERS\|validate_step3_result\|providers_with_keys" crates/scorpio-cli/src/
```

Update each call site to thread both the effective merged provider config and the `copilot_only_selected` flag through, defaulting the latter to `false` until step 3 introduces the bypass UI in the next task.

- [ ] **Step 9: Run tests**

```bash
cargo test -p scorpio-cli cli::setup::steps::tests::keyed_wizard_providers_excludes_copilot -- --exact
cargo test -p scorpio-cli cli::setup::steps::tests::routing_eligible_providers_includes_copilot_when_no_keys -- --exact
cargo test -p scorpio-cli cli::setup::steps::tests::routing_eligible_providers_appends_copilot_after_effective_keyed_providers -- --exact
cargo test -p scorpio-cli cli::setup::steps::tests::validate_step3_result_passes_with_copilot_only_flag -- --exact
```
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/scorpio-cli/src/cli/setup/steps.rs
git commit -m "$(cat <<'EOF'
refactor(setup): split keyed provider prompts from routing eligibility

KEYED_WIZARD_PROVIDERS lists providers whose secrets the wizard prompts for.
eligible_routing_providers always appends Copilot (after keyed providers) and
derives keyed-provider eligibility from the effective merged provider config,
so env-backed credentials behave the same way in setup and runtime.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 20: Add the explicit "continue with Copilot only" Step-3 bypass

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs` (Step-3 prompt loop)

- [ ] **Step 1: Find the Step-3 entry**

```bash
grep -n "fn step3\|step3_\|run_setup\|All providers configured" crates/scorpio-cli/src/cli/setup/steps.rs | head -10
```

- [ ] **Step 2: Identify the prompt loop**

Read the existing `step3_llm_provider_keys` function in `crates/scorpio-cli/src/cli/setup/steps.rs`. Insert the Copilot-only bypass **before** the keyed-provider entry loop and only when no keyed provider is effectively configured (saved config merged with env overrides).

- [ ] **Step 3: Modify the step-3 entry point**

Add before the key-entry loop:

```rust
    // If no keyed provider is effectively configured via saved config merged with
    // env overrides, offer an explicit Copilot-only bypass.
    // an explicit "continue with Copilot only" bypass. Returning
    // `Ok(StepThreeOutcome { copilot_only: true })` means
    // the wizard skipped key entry, Step 4 is shown with Copilot preselected as
    // the only provider choice, and the model-selection step runs once with the
    // chosen Copilot model copied to both quick-thinking and deep-thinking slots
    // unless the user explicitly changes one of them later in the same flow.
    let effective_providers = Config::load_effective_providers_config_from_user_path(
        scorpio_core::settings::user_config_path()
            .map_err(|e| inquire::InquireError::Custom(Box::new(e)))?,
        partial,
    )
    .unwrap_or_default();
    let any_keyed_configured = !providers_with_keys(partial, &effective_providers).is_empty();

    let mut copilot_only = false;
    if !any_keyed_configured {
        let bypass = inquire::Confirm::new(
            "No LLM provider keys found. Continue with GitHub Copilot only?",
        )
        .with_default(true)
        .prompt()?;
        if bypass {
            copilot_only = true;
            return Ok(StepThreeOutcome { copilot_only });
        }
    }
```

(Adjust the surrounding signature and `StepThreeOutcome` to thread the `copilot_only` flag through to step 4.)

- [ ] **Step 4: Define the post-bypass UX explicitly**

Document and implement this operator flow:

- Step 4 still renders so the provider-first wizard mental model stays intact.
- When `copilot_only` is true, Step 4 shows a single selectable provider entry (`Copilot`) already selected for both quick-thinking and deep-thinking tiers.
- Step 4 copy must say that keyed providers were skipped and can be added later by rerunning setup.
- The Copilot model picker runs once and pre-populates both tiers with that model; if the existing setup flow requires per-tier model selection, show the same prefilled Copilot model in both selectors instead of dropping the user into an empty second prompt.
- Add a small pure helper `default_routing_from_step3(outcome: &StepThreeOutcome) -> RoutingDefaults` in `crates/scorpio-cli/src/cli/setup/steps.rs` so the bypass-to-step4 defaults are unit-testable without driving prompts.

- [ ] **Step 5: Remove the now-redundant env-only helper**

`env_has_any_keyed_provider_secret` is no longer needed once Step 3 uses the effective merged provider config. Delete it rather than maintaining two eligibility mechanisms.

- [ ] **Step 6: Add tests for the bypass behavior and the env-backed eligibility path**

```rust
    #[test]
    fn step3_bypass_not_offered_when_effective_env_key_exists() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "test");
        }
        let partial = PartialConfig::default();
        let providers = Config::load_effective_providers_config_from_user_path(
            scorpio_core::settings::user_config_path().unwrap(),
            &partial,
        )
        .unwrap_or_default();
        assert!(!providers_with_keys(&partial, &providers).is_empty());
        unsafe {
            std::env::remove_var("SCORPIO_OPENAI_API_KEY");
        }
    }

    #[test]
    fn copilot_only_bypass_preselects_copilot_for_both_routing_tiers() {
        let outcome = StepThreeOutcome { copilot_only: true };
        let defaults = default_routing_from_step3(&outcome);
        assert_eq!(defaults.quick_provider, ProviderId::Copilot);
        assert_eq!(defaults.deep_provider, ProviderId::Copilot);
        assert!(defaults.keyed_providers_skipped_message);
    }
```

- [ ] **Step 7: Add Xiaomi MiMo to the keyed-provider prompts**

Find the per-provider key-entry prompt loop. Add a branch for `ProviderId::XiaomiMimo`:

```rust
        ProviderId::XiaomiMimo => {
            partial.xiaomimimo_api_key = prompt_optional_secret(
                "Xiaomi MiMo API key",
                partial.xiaomimimo_api_key.as_deref(),
            )?;
        }
```

(Mirror the OpenAI/DeepSeek prompt structure.)

- [ ] **Step 8: Run tests**

```bash
cargo test -p scorpio-cli cli::setup::steps::tests::step3_bypass_not_offered_when_effective_env_key_exists -- --exact
cargo test -p scorpio-cli cli::setup::steps::tests::copilot_only_bypass_preselects_copilot_for_both_routing_tiers -- --exact
```
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/scorpio-cli/src/cli/setup/
git commit -m "$(cat <<'EOF'
feat(setup): add Copilot-only Step-3 bypass + Xiaomi MiMo key prompt

When no keyed provider has a secret in either the saved config or env vars,
Step 3 offers an explicit Confirm to continue with Copilot only. Choosing
the bypass leaves keyed-provider secrets unset and proceeds to Step 4 with
Copilot as the only routing option.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 21: Setup model selection — Copilot static menu + Xiaomi MiMo discovery

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/model_selection.rs`

- [ ] **Step 1: Find the existing entry point**

```bash
grep -n "discover_provider_models_blocking\|fn select_model\|fn run_model_selection" crates/scorpio-cli/src/cli/setup/model_selection.rs | head -10
```

- [ ] **Step 2: Replace `Config::load_effective_runtime` with provider-only load**

Find the call site (~line 221) and replace it with a call to `Config::load_effective_providers_config_from_user_path(...)` that does not require valid `[llm]` routing.

```rust
    // Use the provider-only load path so first-run discovery works before
    // [llm] routing has been chosen and before setup is saved.
    let config_path = match scorpio_core::settings::user_config_path() {
        Ok(path) => path,
        Err(_) => return bootstrap_fallback_outcome(provider),
    };
    let providers_config = Config::load_effective_providers_config_from_user_path(
        config_path,
        partial,
    )
    .unwrap_or_default();
```

(This helper must already accept a `&PartialConfig` from Task 19 Step 4a.)

- [ ] **Step 3: Add Copilot static menu**

For the model picker UI: when `provider == ProviderId::Copilot`, build a `Select` with the curated models from `scorpio_core::providers::factory::COPILOT_CURATED_MODELS` (or its re-exported accessor) plus an `Enter model manually` option. The prompt copy should clarify that the list is a curated setup shortcut and that manual entry remains available for any other supported Copilot model:

```rust
    if provider == ProviderId::Copilot {
        let mut options: Vec<&str> = COPILOT_CURATED_MODELS.iter().copied().collect();
        const MANUAL: &str = "Enter model manually";
        options.push(MANUAL);
        let saved = previously_saved_model.as_deref();
        // If the saved model is in the curated list, select it; else default to manual entry
        // and prefill the saved value.
        let default_index = saved
            .and_then(|s| options.iter().position(|opt| *opt == s))
            .unwrap_or(options.len() - 1); // manual
        let chosen = inquire::Select::new(
            "Copilot model (curated defaults; choose manual entry for any other model)",
            options.clone(),
        )
            .with_starting_cursor(default_index)
            .prompt()?;
        if chosen == MANUAL {
            return Ok(prompt_manual_model_entry(saved));
        }
        return Ok(chosen.to_owned());
    }
```

- [ ] **Step 4: Define Xiaomi MiMo discovery fallback states explicitly**

Before wiring the picker, document the operator-facing outcomes for each discovery result:

- `Listed(models)`: show the discovered list plus `Enter model manually`.
- `Unavailable { reason }` because of custom `base_url`: show a short note that discovery is skipped for trusted-host overrides and go straight to manual entry.
- `Unavailable { reason }` because of invalid key / network / empty results: show the reason inline, preserve any previously saved manual model, and offer manual entry without forcing the user to restart setup.
- When a Xiaomi MiMo trusted-host override is present, show a confirmation note before manual entry that prompts, responses, and the Xiaomi API key will be sent to that configured host.

- [ ] **Step 5: Add Copilot menu coverage tests**

Append to model_selection.rs tests (or use the existing fixture pattern):

```rust
    #[test]
    fn copilot_menu_contains_curated_models_plus_manual() {
        // Use a deterministic version of the menu builder if one exists, or
        // assert on the COPILOT_CURATED_MODELS constant directly.
        assert!(COPILOT_CURATED_MODELS.contains(&"gpt-4o"));
        assert!(COPILOT_CURATED_MODELS.contains(&"claude-sonnet-4"));
        assert!(!COPILOT_CURATED_MODELS.iter().any(|m| m.contains("codex")));
    }
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p scorpio-cli cli::setup::model_selection::tests::copilot_menu_contains_curated_models_plus_manual -- --exact
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/scorpio-cli/src/cli/setup/model_selection.rs
git commit -m "$(cat <<'EOF'
feat(setup): Copilot static model menu + use provider-only config loader

Copilot model selection uses a curated static list with manual-entry fallback.
Discovery no longer bootstraps through Config::load_effective_runtime, which
allowed first-run Copilot-only setup to fail before [llm] routing existed.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9: Copilot OAuth Health Check (step5_health_check)

### Task 22: Add the Copilot identity-binding record + live GitHub authority validator

**Files:**
- Create: `crates/scorpio-core/src/providers/factory/copilot_auth.rs`
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs` (export the new module)

- [ ] **Step 1: Create the module**

Create `crates/scorpio-core/src/providers/factory/copilot_auth.rs`:

```rust
//! Copilot OAuth scope validation and identity-binding record.
//!
//! rig-core 0.36.0 does not surface OAuth scopes from cached grants, and the
//! `bootstrap_token_fingerprint` in rig's `api-key.json` is computed by a
//! process-randomized `DefaultHasher` (not cross-process verifiable). This
//! module therefore relies on a live `GET /user` call against the GitHub authority
//! currently bound in rig's `api-key.json` to confirm identity and inspect the
//! `X-OAuth-Scopes` header.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::error::TradingError;

/// Scorpio-owned identity binding written to `<token_dir>/scorpio-identity.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScorpioIdentityBinding {
    /// Numeric GitHub account ID (mandatory; survives login renames).
    pub github_id: u64,
    /// GitHub login at time of authorization (display only — never used as the primary identity key).
    pub github_login: String,
    /// Unix timestamp (seconds) at which this binding was written.
    pub written_at: i64,
    /// Canonical GitHub user-info authority used when validating this grant
    /// (for github.com this is `https://api.github.com`).
    pub github_api_base: String,
}

/// Read the identity binding from the token directory.
pub fn read_binding(token_dir: &Path) -> Result<ScorpioIdentityBinding> {
    let path = token_dir.join("scorpio-identity.json");
    verify_copilot_secret_file_secure(&path)?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("identity binding missing at {}", path.display()))?;
    let parsed: ScorpioIdentityBinding =
        serde_json::from_str(&raw).context("identity binding is malformed JSON")?;
    if parsed.github_id == 0 {
        return Err(anyhow::anyhow!(
            "identity binding missing github_id (must be a non-zero numeric account ID)"
        ));
    }
    Ok(parsed)
}

/// Write the identity binding atomically with `0o600` permissions on Unix.
pub fn write_binding(token_dir: &Path, binding: &ScorpioIdentityBinding) -> Result<()> {
    let path = token_dir.join("scorpio-identity.json");
    let json = serde_json::to_string_pretty(binding)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perms)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Verify that a Copilot secret/cache file is a regular non-symlink file owned by
/// the current user.
///
/// The token directory is the primary confidentiality boundary in this slice.
/// We still reject non-regular files and foreign-owned files here, but we do not
/// fail solely because rig wrote broader file modes inside an already-secure
/// Scorpio-owned token directory.
pub fn verify_copilot_secret_file_secure(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("secret file missing at {}", path.display()))?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "secret file at {} must be a regular non-symlink file",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            return Err(anyhow::anyhow!(
                "secret file at {} is not owned by the current user",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Required GitHub OAuth scope on the Copilot bootstrap token.
pub const REQUIRED_SCOPE: &str = "read:user";
/// Additional scopes tolerated for compatibility if GitHub includes them on a
/// valid Copilot device-flow grant.
pub const ALLOWED_SCOPE_EXTRAS: &[&str] = &[];

/// Live identity returned by `GET <github_api_base>/user`.
#[derive(Debug)]
pub struct GitHubIdentity {
    pub id: u64,
    pub login: String,
    pub scopes: Vec<String>,
}

/// Resolve the GitHub user-info base URL from rig's `api-key.json` cache file
/// (`endpoints.api`).
/// Only `https://api.github.com` plus an explicit allowlist of approved GitHub
/// Enterprise API base URLs are accepted here.
pub fn read_github_api_base(token_dir: &Path) -> Result<String, TradingError> {
    let path = token_dir.join("api-key.json");
    verify_copilot_secret_file_secure(&path)
        .map_err(|e| TradingError::Config(anyhow::anyhow!(e.to_string())))?;
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        TradingError::Config(anyhow::anyhow!("failed to read {}: {e}", path.display()))
    })?;

    #[derive(Deserialize, Default)]
    struct ApiKeyFile {
        #[serde(default)]
        endpoints: ApiKeyEndpoints,
    }

    #[derive(Deserialize, Default)]
    struct ApiKeyEndpoints {
        #[serde(default)]
        api: Option<String>,
    }

    let parsed: ApiKeyFile = serde_json::from_str(&raw).map_err(|e| {
        TradingError::Config(anyhow::anyhow!("failed to parse {}: {e}", path.display()))
    })?;

    let candidate = parsed
        .endpoints
        .api
        .unwrap_or_else(|| "https://api.github.com".to_owned());
    let url = url::Url::parse(candidate.trim()).map_err(|e| {
        TradingError::Config(anyhow::anyhow!("invalid GitHub API base URL in api-key.json: {e}"))
    })?;
    if url.scheme() != "https" {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GitHub API base URL must use https"
        )));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GitHub API base URL must not contain userinfo, query, or fragment"
        )));
    }
    let host = url.host_str().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!("GitHub API base URL must include a host"))
    })?;
    let normalized = url.as_str().trim_end_matches('/').to_owned();
    if url.path() != "/" && !url.path().is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GitHub API base URL must not contain a path component"
        )));
    }
    let allowed = normalized == "https://api.github.com"
        || approved_github_api_bases().iter().any(|base| *base == normalized);
    if !allowed {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GitHub API base URL {normalized:?} is not in the approved GitHub/GHE allowlist"
        )));
    }
    Ok(normalized)
}

fn approved_github_api_bases() -> &'static [&'static str] {
    &[]
}

/// Call `GET <github_api_base>/user` with the given access token, returning
/// the numeric ID, login, and the parsed `X-OAuth-Scopes` header.
pub async fn fetch_github_identity(
    github_api_base: &str,
    access_token: &str,
) -> Result<GitHubIdentity, TradingError> {
    let client = reqwest::Client::builder()
        .user_agent("scorpio-analyst")
        .build()
        .map_err(|e| {
            TradingError::Config(anyhow::anyhow!("reqwest client build failed: {e}"))
        })?;
    let resp = client
        .get(format!("{github_api_base}/user"))
        .bearer_auth(access_token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| TradingError::Config(anyhow::anyhow!("GET /user failed: {e}")))?;

    let scopes_header = resp
        .headers()
        .get("X-OAuth-Scopes")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if !resp.status().is_success() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "GET /user returned status {}",
            resp.status()
        )));
    }

    #[derive(Deserialize)]
    struct UserResponse {
        id: u64,
        login: String,
    }
    let body: UserResponse = resp
        .json()
        .await
        .map_err(|e| TradingError::Config(anyhow::anyhow!("GET /user body parse: {e}")))?;

    let scopes: Vec<String> = scopes_header
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    Ok(GitHubIdentity {
        id: body.id,
        login: body.login,
        scopes,
    })
}

/// Reject the cached grant unless it contains `REQUIRED_SCOPE` and no scopes
/// outside the explicit compatibility allowlist.
pub fn validate_scope(scopes: &[String]) -> Result<(), TradingError> {
    if scopes.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "X-OAuth-Scopes header was empty; refusing to trust this grant"
        )));
    }
    if !scopes.iter().any(|scope| scope == REQUIRED_SCOPE) {
        return Err(TradingError::Config(anyhow::anyhow!(
            "Copilot bootstrap is missing required scope {REQUIRED_SCOPE:?}"
        )));
    }
    for scope in scopes {
        if scope != REQUIRED_SCOPE && !ALLOWED_SCOPE_EXTRAS.iter().any(|allowed| *allowed == scope)
        {
            return Err(TradingError::Config(anyhow::anyhow!(
                "Copilot bootstrap has unexpected scope {scope:?}; required scope is {REQUIRED_SCOPE:?} and tolerated extras are {ALLOWED_SCOPE_EXTRAS:?}"
            )));
        }
    }
    Ok(())
}

/// Read the access token from rig's managed cache file at `<token_dir>/access-token`.
pub fn read_access_token(token_dir: &Path) -> Result<String, TradingError> {
    let path = token_dir.join("access-token");
    verify_copilot_secret_file_secure(&path)
        .map_err(|e| TradingError::Config(anyhow::anyhow!(e.to_string())))?;
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        TradingError::Config(anyhow::anyhow!(
            "failed to read access token at {}: {e}",
            path.display()
        ))
    })?;
    let trimmed = raw.trim().to_owned();
    if trimmed.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "access token file at {} is empty",
            path.display()
        )));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_scope_accepts_read_user_only() {
        assert!(validate_scope(&["read:user".to_owned()]).is_ok());
    }

    #[test]
    fn validate_scope_rejects_empty() {
        assert!(validate_scope(&[]).is_err());
    }

    #[test]
    fn validate_scope_rejects_missing_read_user() {
        assert!(validate_scope(&["other".to_owned()]).is_err());
    }

    #[test]
    fn validate_scope_rejects_repo_scope() {
        assert!(validate_scope(&["read:user".to_owned(), "repo".to_owned()]).is_err());
    }

    #[test]
    fn validate_scope_rejects_unexpected_scope_even_when_not_in_old_denylist() {
        assert!(validate_scope(&["read:user".to_owned(), "read:org".to_owned()]).is_err());
    }

    #[test]
    fn binding_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let binding = ScorpioIdentityBinding {
            github_id: 42,
            github_login: "octocat".to_owned(),
            written_at: 1234567890,
            github_api_base: "https://api.github.com".to_owned(),
        };
        write_binding(dir.path(), &binding).unwrap();
        let loaded = read_binding(dir.path()).unwrap();
        assert_eq!(loaded, binding);
    }

    #[test]
    fn binding_with_zero_id_rejected_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let binding = ScorpioIdentityBinding {
            github_id: 0,
            github_login: "x".to_owned(),
            written_at: 0,
            github_api_base: "https://api.github.com".to_owned(),
        };
        std::fs::write(
            dir.path().join("scorpio-identity.json"),
            serde_json::to_string(&binding).unwrap(),
        )
        .unwrap();
        let err = read_binding(dir.path()).unwrap_err();
        assert!(err.to_string().contains("github_id"));
    }
}
```

- [ ] **Step 2: Re-export the module**

Edit `crates/scorpio-core/src/providers/factory/mod.rs` to add:

```rust
pub mod copilot_auth;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p scorpio-core providers::factory::copilot_auth::tests::validate_scope_accepts_read_user_only -- --exact
cargo test -p scorpio-core providers::factory::copilot_auth::tests::binding_round_trip -- --exact
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/
git commit -m "$(cat <<'EOF'
feat(providers): add Copilot identity-binding + GET /user scope validator

Adds ScorpioIdentityBinding (numeric GitHub ID + login + timestamp + GitHub API authority)
and a fetch_github_identity helper that calls GET /user against the bound authority and
parses X-OAuth-Scopes.
validate_scope rejects empty scopes, missing read:user, and any disallowed
broader scope (repo, write:*, admin:*, delete_*, user:email).

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 23: Wire Copilot OAuth + identity validation into `step5_health_check`

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs`
- Modify: `crates/scorpio-cli/src/cli/setup/mod.rs` (only if imports need to change)
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs` test module for the injected helper seam

- [ ] **Step 1: Find `step5_health_check`**

```bash
grep -rn "fn step5_health_check\|step5_health_check\b" crates/scorpio-cli/
```

- [ ] **Step 2: Read its current behavior**

The current `step5_health_check` path is synchronous: it loads `Config::load_effective_runtime(partial.clone())`, then passes `run_single_health_check(&cfg)` into `run_health_check_loop(...)`. `run_single_health_check` builds a current-thread Tokio runtime, calls `cfg.is_analysis_ready()`, then constructs handles with `create_completion_model(...)` and probes them with `prompt_with_retry(...)`. Copilot setup must preserve that retry/save-anyway loop while swapping in an explicit authorize + identity-validation path.

- [ ] **Step 3: Write a unit test for the new helper**

Create `step5_validate_copilot_auth(token_dir)` as a separate function so it can be tested independently:

```rust
    #[tokio::test]
    async fn step5_validate_copilot_auth_writes_identity_binding_on_success() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            dir.path().join("api-key.json"),
            r#"{"endpoints":{"api":"https://api.github.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                dir.path().join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }

        step5_validate_copilot_auth_with(dir.path(), |_base, _token| async {
            Ok(copilot_auth::GitHubIdentity {
                id: 42,
                login: "octocat".to_owned(),
                scopes: vec!["read:user".to_owned()],
            })
        })
        .await
        .unwrap();

        let binding = copilot_auth::read_binding(dir.path()).unwrap();
        assert_eq!(binding.github_id, 42);
        assert_eq!(binding.github_login, "octocat");
        assert_eq!(binding.github_api_base, "https://api.github.com");
    }
```

- [ ] **Step 4: Implement `step5_validate_copilot_auth`**

```rust
async fn step5_validate_copilot_auth(token_dir: &std::path::Path) -> anyhow::Result<()> {
    step5_validate_copilot_auth_with(token_dir, copilot_auth::fetch_github_identity).await
}

async fn step5_validate_copilot_auth_with<F, Fut>(
    token_dir: &std::path::Path,
    fetch_identity: F,
) -> anyhow::Result<()>
where
    F: Fn(&str, &str) -> Fut,
    Fut: std::future::Future<Output = Result<copilot_auth::GitHubIdentity, TradingError>>,
{
    use scorpio_core::providers::factory::copilot_auth;

    // 1. Read the access token rig cached.
    let access = copilot_auth::read_access_token(token_dir)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 2. Resolve the GitHub authority rig bound for this Copilot grant.
    let github_api_base = copilot_auth::read_github_api_base(token_dir)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 3. Confirm identity + scope via GET /user.
    let identity = fetch_identity(&github_api_base, &access)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    copilot_auth::validate_scope(&identity.scopes)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    // 4. Write the identity-binding record.
    let binding = copilot_auth::ScorpioIdentityBinding {
        github_id: identity.id,
        github_login: identity.login,
        written_at: chrono::Utc::now().timestamp(),
        github_api_base,
    };
    copilot_auth::write_binding(token_dir, &binding)?;

    Ok(())
}
```

- [ ] **Step 5: Wire it into the existing `step5_health_check`**

When the configured provider is `copilot`, preserve the existing `run_health_check_loop(...)` behavior but swap in a Copilot-specific one-shot helper:

```rust
fn run_single_copilot_health_check(
    tiers: &[ModelTier],
    cfg: &scorpio_core::config::Config,
    rate_limiters: &scorpio_core::rate_limit::ProviderRateLimiters,
    token_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for Copilot health check")?;

    for &tier in tiers {
        let handle = scorpio_core::providers::factory::create_completion_model_with_copilot(
            tier,
            &cfg.llm,
            &cfg.providers,
            rate_limiters,
            CopilotAuthMode::InteractiveSetup,
            token_dir,
        )
        .map_err(|e| anyhow::anyhow!("failed to create Copilot completion model: {e}"))?;

        runtime.block_on(async {
            // rig-core 0.36.0 authenticates lazily here, not during builder.build().
            handle.authorize_copilot().await?;
            step5_validate_copilot_auth(token_dir).await?;

            let agent = scorpio_core::providers::factory::build_agent(&handle, "");
            scorpio_core::providers::factory::prompt_with_retry(
                &agent,
                "Hello",
                Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS),
                &RetryPolicy::default(),
            )
            .await
        })?;
    }

    Ok(())
}

fn effective_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider == "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider == "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

fn configured_non_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider != "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider != "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

pub fn step5_health_check(partial: &PartialConfig) -> anyhow::Result<bool> {
    let deep_provider = partial.deep_thinking_provider.as_deref().unwrap_or("");
    let deep_model = partial.deep_thinking_model.as_deref().unwrap_or("");
    println!("Sending \"Hello\" to deep-thinking provider ({deep_provider} / {deep_model})...");

    let cfg = scorpio_core::config::Config::load_effective_runtime(partial.clone())?;

    let copilot_tiers = effective_copilot_tiers(&cfg);
    if !copilot_tiers.is_empty() {
        let consent = inquire::Confirm::new(
            "Copilot setup will request the GitHub `read:user` OAuth scope. Continue?",
        )
        .with_default(true)
        .prompt()?;
        if !consent {
            return Ok(false);
        }

        let token_dir = scorpio_core::settings::ensure_copilot_token_dir()?;
        let rate_limiters =
            scorpio_core::rate_limit::ProviderRateLimiters::from_config(&cfg.providers);

        return run_health_check_loop(
            || {
                run_single_copilot_health_check(&copilot_tiers, &cfg, &rate_limiters, &token_dir)?;

                check_selected_model_tiers(configured_non_copilot_tiers(&cfg), |tier| {
                    let handle = scorpio_core::providers::factory::create_completion_model(
                        tier,
                        &cfg.llm,
                        &cfg.providers,
                        &rate_limiters,
                    )
                    .map_err(|e| anyhow::anyhow!("failed to create completion model: {e}"))?;

                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .context("failed to build runtime for mixed-provider health check")?;

                    runtime
                        .block_on(async {
                            let agent = scorpio_core::providers::factory::build_agent(&handle, "");
                            scorpio_core::providers::factory::prompt_with_retry(
                                &agent,
                                "Hello",
                                Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS),
                                &RetryPolicy::default(),
                            )
                            .await
                        })
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!(e))
                })
            },
            |error| {
                eprintln!(
                    "✗ Health check failed: {}",
                    scorpio_core::providers::factory::sanitize_error_summary(&error.to_string())
                );
            },
            || {
                inquire::Confirm::new("Retry health check?")
                    .with_default(true)
                    .prompt()
                    .map_err(anyhow::Error::from)
            },
            || {
                inquire::Confirm::new("Save config anyway?")
                    .with_default(false)
                    .prompt()
                    .map_err(anyhow::Error::from)
            },
        );
    }

    run_health_check_loop(
        || run_single_health_check(&cfg),
        |error| {
            eprintln!(
                "✗ Health check failed: {}",
                scorpio_core::providers::factory::sanitize_error_summary(&error.to_string())
            );
        },
        || {
            inquire::Confirm::new("Retry health check?")
                .with_default(true)
                .prompt()
                .map_err(anyhow::Error::from)
        },
        || {
            inquire::Confirm::new("Save config anyway?")
                .with_default(false)
                .prompt()
                .map_err(anyhow::Error::from)
        },
    )
}
```

(Adapt the surrounding signatures and error types to match the existing `step5_health_check` shape.)

    - [ ] **Step 6: Use the `effective_copilot_tiers(cfg)` helper for routed-tier selection**

```rust
// Reuse the existing helper from above:
// fn effective_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier>
//
// This task intentionally uses the full routed-tier set rather than collapsing to
// the first matching tier, so mixed quick/deep Copilot routing stays correct.
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p scorpio-cli cli::setup::steps::tests::step5_validate_copilot_auth_writes_identity_binding_on_success -- --exact
cargo test -p scorpio-core providers::factory::copilot_auth::tests::binding_round_trip -- --exact
```
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/scorpio-cli/src/cli/setup/ crates/scorpio-core/src/providers/factory/
git commit -m "$(cat <<'EOF'
feat(setup): wire Copilot authorization + identity binding into step5_health_check

Setup flow:
1. Show OAuth scope boundary and require explicit consent.
2. ensure_copilot_token_dir() with 0o700 perms.
3. Build a Copilot handle for whichever tier is actually routed to Copilot and call `authorize_copilot()` inside the existing retry/save-anyway loop.
4. Read access token, resolve the bound GitHub authority from api-key.json, call GET /user, and validate X-OAuth-Scopes.
5. Write scorpio-identity.json with the numeric GitHub account ID and bound GitHub API authority.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

### Task 24: Cached auth reuse validation in runtime path

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/client.rs` (in `create_completion_model_with_copilot` or a new helper invoked before client construction)
- Modify: `crates/scorpio-core/src/app/mod.rs` (`AnalysisRuntime::new` or a small async helper it calls)
- Modify: `crates/scorpio-core/src/config.rs` (`Config::is_analysis_ready`)
- Test: `crates/scorpio-core/src/providers/factory/client.rs`, `crates/scorpio-core/src/config.rs`, `crates/scorpio-core/tests/app_runtime.rs`

- [ ] **Step 1: Add a cache-validation step to `NonInteractiveRuntime`**

In `create_completion_model_with_copilot`, before constructing the client, add (when `mode == NonInteractiveRuntime`):

```rust
        // Verify directory permissions.
        crate::settings::verify_copilot_token_dir_secure(token_dir)
            .map_err(|e| config_error(&format!("token directory rejected: {e}")))?;

        // Verify identity binding exists with a valid numeric ID.
        let binding = copilot_auth::read_binding(token_dir)
            .map_err(|e| config_error(&format!("identity binding rejected: {e}")))?;

        // Re-read the GitHub authority from rig's cache and require it to match the
        // previously bound authority. GitHub Enterprise-managed redirects are allowed,
        // but drift must force a fresh setup run.
        let github_api_base = copilot_auth::read_github_api_base(token_dir)
            .map_err(|e| config_error(&format!("github authority rejected: {e}")))?;
        if github_api_base != binding.github_api_base {
            return Err(config_error(
                "copilot cached authority changed; clear the Copilot cache and rerun `scorpio setup`",
            ));
        }

        // Do not perform the live GET /user check inside the synchronous client
        // factory with block_on. Instead, move live identity/scope revalidation into
        // an async readiness/bootstrap helper invoked by AnalysisRuntime::try_new(...)
        // and Config::is_analysis_ready(), so runtime still fails closed before any
        // analysis run begins.
```

Decision: perform the live `GET /user` revalidation once per runtime bootstrap in `AnalysisRuntime::new(...)` and once in `Config::is_analysis_ready()` rather than inside `create_completion_model_with_copilot(...)`. The factory remains responsible for local token-dir/regular-file/authority checks; bootstrap/readiness remain responsible for async identity and scope validation.

- [ ] **Step 2: Add async Copilot bootstrap helpers to core runtime assembly**

Add a shared async helper in `crates/scorpio-core/src/config.rs` plus a sync wrapper used by `Config::is_analysis_ready()`. Also add narrowly-scoped test-only injection helpers that delegate to the real entrypoints so the runtime/config tests below can substitute a fake validator without widening the production API surface:

```rust
async fn validate_copilot_runtime_auth_if_configured(cfg: &Config) -> anyhow::Result<()> {
    // For each tier routed to Copilot:
    // 1. Resolve Scorpio's token dir.
    // 2. Verify 0o700 ownership/permissions.
    // 3. Read scorpio-identity.json.
    // 4. Read api-key.json authority.
    // 5. Read access-token.
    // 6. Call GET <github_api_base>/user.
    // 7. Require matching github_id + allowed scopes.
}

fn validate_copilot_runtime_auth_if_configured_blocking(cfg: &Config) -> anyhow::Result<()> {
    // Build a current-thread runtime locally and block only in this sync wrapper.
    // Config::is_analysis_ready() stays sync for existing CLI call sites.
}

#[cfg(any(test, feature = "test-helpers"))]
async fn validate_copilot_runtime_auth_with<F, Fut>(
    cfg: &Config,
    fetch_identity: F,
) -> anyhow::Result<()>
where
    F: Fn(&str, &str) -> Fut,
    Fut: std::future::Future<Output = Result<copilot_auth::GitHubIdentity, TradingError>>,
{
    // Same logic as validate_copilot_runtime_auth_if_configured, but with the
    // GitHub identity fetch injected for hermetic tests.
}

#[cfg(any(test, feature = "test-helpers"))]
impl Config {
    pub fn is_analysis_ready_with_copilot_validator<F>(&self, validate: F) -> anyhow::Result<()>
    where
        F: Fn(&Config) -> anyhow::Result<()>,
    {
        validate(self)?;
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

        Ok(())
    }
}

#[cfg(any(test, feature = "test-helpers"))]
impl AnalysisRuntime {
    pub async fn new_with_copilot_validator<F, Fut>(cfg: Config, validate: F) -> anyhow::Result<Self>
    where
        F: Fn(&Config) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        validate(&cfg).await?;
        // Same body as AnalysisRuntime::new, but skip the production validator call
        // so the injected seam is exercised exactly once.
    }
}
```

Call the async helper from `AnalysisRuntime::new(...)` before either `create_completion_model(...)` call. Call the sync wrapper from `Config::is_analysis_ready()` before returning success when either tier is routed to Copilot. Do **not** make `Config::is_analysis_ready()` async in this slice; `crates/scorpio-cli/src/cli/analyze.rs` and `crates/scorpio-cli/src/cli/setup/steps.rs` currently call it from synchronous code paths.

- [ ] **Step 3: Add tests**

```rust
    #[test]
    fn runtime_mode_rejects_when_identity_binding_missing() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        // Pretend rig cache exists.
        std::fs::write(token_dir.join("access-token"), "fake-token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"endpoints":{"api":"https://api.github.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        // No scorpio-identity.json.

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        let err = result.unwrap_err();
        assert!(err.to_string().contains("identity"));
    }

    #[test]
    fn runtime_mode_rejects_when_bound_github_authority_changes() {
        let dir = tempfile::tempdir().unwrap();
        let token_dir = dir.path().join("github_copilot");
        std::fs::create_dir_all(&token_dir).unwrap();
        std::fs::write(token_dir.join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"endpoints":{"api":"https://ghe.example.com/api/v3"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        copilot_auth::write_binding(
            &token_dir,
            &copilot_auth::ScorpioIdentityBinding {
                github_id: 42,
                github_login: "octocat".to_owned(),
                written_at: 0,
                github_api_base: "https://api.github.com".to_owned(),
            },
        )
        .unwrap();

        let mut cfg = sample_llm_config();
        cfg.quick_thinking_provider = "copilot".to_owned();
        cfg.quick_thinking_model = "gpt-4o".to_owned();

        let result = create_completion_model_with_copilot(
            ModelTier::QuickThinking,
            &cfg,
            &ProvidersConfig::default(),
            &ProviderRateLimiters::default(),
            CopilotAuthMode::NonInteractiveRuntime,
            &token_dir,
        );
        let err = result.unwrap_err();
        assert!(err.to_string().contains("authority"));
    }

    fn sample_runtime_config_with_copilot() -> Config {
        let mut cfg = Config::default();
        cfg.llm.quick_thinking_provider = "copilot".to_owned();
        cfg.llm.quick_thinking_model = "gpt-4o".to_owned();
        cfg.llm.deep_thinking_provider = "openai".to_owned();
        cfg.llm.deep_thinking_model = "gpt-4o-mini".to_owned();
        cfg
    }

    #[tokio::test]
    async fn runtime_mode_rejects_when_live_github_identity_mismatches_binding() {
        let cfg = sample_runtime_config_with_copilot();
        let err = validate_copilot_runtime_auth_with(&cfg, |_base, _token| async {
            Ok(copilot_auth::GitHubIdentity {
                id: 99,
                login: "wrong-user".to_owned(),
                scopes: vec!["read:user".to_owned()],
            })
        })
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("bound GitHub account"));
    }

    #[tokio::test]
    async fn runtime_mode_rejects_when_live_scopes_exceed_allowed_set() {
        let cfg = sample_runtime_config_with_copilot();
        let err = validate_copilot_runtime_auth_with(&cfg, |_base, _token| async {
            Ok(copilot_auth::GitHubIdentity {
                id: 42,
                login: "octocat".to_owned(),
                scopes: vec!["read:user".to_owned(), "repo".to_owned()],
            })
        })
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("scope"));
    }

    #[tokio::test]
    async fn analysis_runtime_new_revalidates_copilot_before_handle_creation() {
        let cfg = sample_runtime_config_with_copilot();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let result = AnalysisRuntime::new_with_copilot_validator(cfg, move |_cfg| {
            let calls = calls_clone.clone();
            async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn is_analysis_ready_revalidates_copilot_before_reporting_success() {
        let cfg = sample_runtime_config_with_copilot();
        let err = cfg
            .is_analysis_ready_with_copilot_validator(|_cfg| Err(anyhow::anyhow!("scope mismatch")))
            .unwrap_err();
        assert!(format!("{err:#}").contains("scope mismatch"));
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p scorpio-core providers::factory::client::tests::runtime_mode_rejects_when_identity_binding_missing -- --exact
cargo test -p scorpio-core providers::factory::client::tests::runtime_mode_rejects_when_bound_github_authority_changes -- --exact
cargo test -p scorpio-core config::tests::runtime_mode_rejects_when_live_github_identity_mismatches_binding -- --exact
cargo test -p scorpio-core config::tests::runtime_mode_rejects_when_live_scopes_exceed_allowed_set -- --exact
cargo test -p scorpio-core --features test-helpers --test app_runtime analysis_runtime_new_revalidates_copilot_before_handle_creation -- --exact
cargo test -p scorpio-core config::tests::is_analysis_ready_revalidates_copilot_before_reporting_success -- --exact
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/client.rs crates/scorpio-core/src/app/mod.rs crates/scorpio-core/src/config.rs
git commit -m "feat(providers): runtime Copilot reuse revalidates identity, scopes, and authority

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 10: Error Sanitization

### Task 25: Extend `redact_credentials` with GitHub OAuth patterns

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/error.rs`

- [ ] **Step 1: Read existing redaction patterns**

```bash
sed -n '140,180p' crates/scorpio-core/src/providers/factory/error.rs
```

- [ ] **Step 2: Write failing tests**

Append:

```rust
    #[test]
    fn redact_credentials_redacts_github_token_prefixes() {
        for prefix in ["ghu_", "gho_", "ghr_", "github_pat_"] {
            let raw = format!("token leaked {prefix}abcdef1234567890ABCDEF");
            let cleaned = redact_credentials(&raw);
            assert!(!cleaned.contains("abcdef1234567890ABCDEF"),
                "raw {prefix}-prefixed token leaked: {cleaned}");
        }
    }

    #[test]
    fn redact_credentials_redacts_device_user_code() {
        let raw = "Enter code ABCD-1234 at the prompt";
        let cleaned = redact_credentials(raw);
        assert!(!cleaned.contains("ABCD-1234"));
    }

    #[test]
    fn redact_credentials_redacts_verification_uri() {
        let raw = "Visit https://github.com/login/device to verify";
        let cleaned = redact_credentials(raw);
        assert!(!cleaned.contains("https://github.com/login/device"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p scorpio-core providers::factory::error::tests::redact_credentials_redacts_github_token_prefixes -- --exact
```
Expected: FAIL.

- [ ] **Step 4: Add the redaction patterns**

In `redact_credentials`, append regex/string match arms for:
- Token prefixes: `(ghu_|gho_|ghr_|github_pat_)[A-Za-z0-9_]+` → `[REDACTED]`
- 8-char hyphenated user code: `[A-Z0-9]{4}-[A-Z0-9]{4}` → `[REDACTED]`
- Verification URI: `https://github.com/login/device` → `[REDACTED_URL]`

Use the same regex/replace approach as the existing patterns.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p scorpio-core providers::factory::error::tests::redact_credentials_redacts_github_token_prefixes -- --exact
cargo test -p scorpio-core providers::factory::error::tests::redact_credentials_redacts_device_user_code -- --exact
cargo test -p scorpio-core providers::factory::error::tests::redact_credentials_redacts_verification_uri -- --exact
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scorpio-core/src/providers/factory/error.rs
git commit -m "feat(error): redact GitHub OAuth token prefixes and device codes

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 11: Public Docs and `.env.example`

### Task 26: Update README with Copilot and Xiaomi MiMo

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Read current provider documentation**

```bash
grep -n "Provider\|copilot\|deepseek\|openrouter" README.md | head -30
```

- [ ] **Step 2: Add Copilot back as a supported provider**

In the supported-providers table or list, re-add Copilot with a note that it uses native rig-core OAuth (not the deleted custom ACP runtime):

```markdown
- **GitHub Copilot** — OAuth/device-flow (no API key required). Run `scorpio setup` and select Copilot to authorize via GitHub. Token cache lives at `~/.scorpio-analyst/github_copilot/`.
- Manual model entry remains available for Copilot even though setup starts from a curated list of known-good defaults.
```

- [ ] **Step 3: Add Xiaomi MiMo**

```markdown
- **Xiaomi MiMo** — Native Scorpio provider backed by rig's Xiaomi MiMo client. Set `SCORPIO_XIAOMIMIMO_API_KEY` or run `scorpio setup`. Advanced `base_url` overrides are restricted to trusted HTTPS hosts (or loopback HTTP for local dev), and prompts, responses, and the API key are sent to that configured host.
```

- [ ] **Step 4: Update any provider-name lists**

```bash
grep -n '"openai"\|"anthropic"\|"gemini"\|"openrouter"\|"deepseek"' README.md
```

Add `"copilot"` and `"xiaomimimo"` to each list.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: re-add Copilot and add Xiaomi MiMo to supported providers

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

### Task 27: Update `.env.example`

**Files:**
- Modify: `.env.example`

- [ ] **Step 1: Add the Xiaomi MiMo key entry**

```bash
grep -n "API_KEY" .env.example
```

Add the `SCORPIO_XIAOMIMIMO_API_KEY=` entry alongside the other LLM provider keys. **Do not add a Copilot env key** (Copilot uses OAuth, not env-managed secrets).

- [ ] **Step 2: Commit**

```bash
git add .env.example
git commit -m "docs(env): add SCORPIO_XIAOMIMIMO_API_KEY example

Copilot is intentionally absent — it uses OAuth/device flow, not an API key.

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

## Phase 12: Verification

### Task 28: Run the full repo verification suite

- [ ] **Step 1: Format check**

Run:
```bash
cargo fmt -- --check
```
Expected: clean.

- [ ] **Step 2: Clippy with warnings as errors**

Run:
```bash
cargo clippy --workspace --all-targets -- -D warnings
```
Expected: no warnings.

- [ ] **Step 3: Full test suite**

Run:
```bash
cargo nextest run --workspace --all-features --locked --no-fail-fast
```
Expected: all tests pass.

- [ ] **Step 4: Manual smoke test — fresh setup with Copilot only**

```bash
env -i HOME=$(mktemp -d) PATH="$PATH" TERM="${TERM:-xterm-256color}" cargo run -p scorpio-cli -- setup
```

Manually verify:
- Step 3 offers the "continue with Copilot only" bypass when no env keys are set.
- Choosing the bypass proceeds to step 4 with Copilot preselected for both tiers as the only option.
- Step 5 prompts for OAuth consent, then `authorize_copilot()` either reuses cached auth or opens GitHub's device flow, and writes `scorpio-identity.json` after success.
- Step 5 writes `scorpio-identity.json` only after validating the live GitHub account ID, granted scopes, and bound GitHub API authority.

- [ ] **Step 5: Manual smoke test — Xiaomi MiMo key entry**

Set `SCORPIO_XIAOMIMIMO_API_KEY=test-key` and rerun setup; verify Xiaomi MiMo appears in the keyed-provider list and discovery falls back to manual entry (since the test key is invalid).

- [ ] **Step 6: Final sanity commit (if any quality fixes were needed)**

If clippy/format produced fixes:
```bash
git add -A && git commit -m "chore: cargo fmt + clippy fixes for new providers"
```

---

## Self-Review Checklist (before reporting plan complete)

- [ ] Every spec section maps to at least one task above (provider identity, config validation, settings, factory client, factory agent, rate limiter, setup wizard, model selection, discovery, OAuth flow, error sanitization, docs).
- [ ] No task contains "TBD", "implement later", "similar to Task N without showing the code", or other placeholder language.
- [ ] Type names are consistent: `CopilotModel`, `XiaomiMimoModel`, `CopilotAuthMode`, `ProviderClient::Copilot`, `LlmAgentInner::Copilot`.
- [ ] Function names are consistent: `validate_xiaomimimo_base_url`, `create_completion_model_with_copilot`, `CompletionModelHandle::authorize_copilot`, `eligible_routing_providers(partial, effective_providers)`, `validate_step3_result(partial, effective_providers, copilot_only_selected)`, `step5_validate_copilot_auth`, `effective_copilot_tiers`, and `read_github_api_base`.
- [ ] Commit steps are present at useful checkpoints, but adjacent tasks may be squashed into larger phase-level commits.
- [ ] Migration step (Phase 0) runs first and removes `STALE_COPILOT_PROVIDER_MARKER` before `ProviderId::Copilot` is added.
- [ ] All tests use `#[test]` or `#[tokio::test]` and have explicit assertions.
- [ ] Every single-test `cargo test` command includes the `-p` crate flag and `--exact`; broader verification uses the repo-standard `fmt`, `clippy`, and `nextest` commands without pretending that an exact-name filter is a suite run.

---

**Plan complete.** Ready for execution.
