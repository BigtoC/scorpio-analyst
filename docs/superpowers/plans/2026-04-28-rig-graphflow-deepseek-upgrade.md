# Rig-Core, Graph-Flow, and DeepSeek Provider Implementation Plan

> **For agentic workers:** REQUIRED: Use `@superpowers:subagent-driven-development` (if subagents available) or `@superpowers:executing-plans` to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the workspace to `graph-flow 0.5.1` and `rig-core 0.35.0`, add DeepSeek as a first-class provider across runtime/config/setup/docs, and preserve Scorpio's existing prompt/chat wrapper semantics.

**Architecture:** Keep provider construction routed through the existing seams in `crates/scorpio-core/src/providers/`. Land the native DeepSeek provider through the same config, settings, rate-limit, and factory paths as the other keyed HTTP providers, localize the `rig-core 0.35.0` chat-history change inside `crates/scorpio-core/src/providers/factory/agent.rs`, and keep DeepSeek on Scorpio's native typed-output analyst path alongside OpenAI, Anthropic, Gemini, and Copilot. Only OpenRouter should retain the unconditional text-fallback path, and Gemini should retain its schema-violation fallback.

**Tech Stack:** Rust 2024, Cargo workspace, `rig-core 0.35.0`, `graph-flow 0.5.1`, `tokio`, `serde`, `inquire`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

**Spec:** `docs/superpowers/specs/2026-04-28-rig-graphflow-deepseek-design.md`
**Worktree:** Execute from `feature/enrich-news-sources`. Confirm with `git worktree list` before starting.

## Guardrails

- Use `@superpowers:subagent-driven-development` for execution and `@superpowers:verification-before-completion` before declaring the upgrade done.
- Do not add an OpenAI-compat alias layer for DeepSeek. The provider must stay explicitly named `deepseek` everywhere Scorpio exposes provider identity.
- Do not add DeepSeek model defaults or recommendations in the setup wizard. Model IDs stay manual text input.
- Keep `LlmAgent::{chat, chat_details}` as Scorpio's stable internal interface even if the `rig-core` implementation changes underneath.
- If the focused `graph-flow` smoke slice passes immediately after the version bump, do not edit workflow/graph code just to “touch” the dependency.
- In `crates/scorpio-core/src/config.rs` and `crates/scorpio-core/src/providers/factory/client.rs`, keep the new DeepSeek-specific tests and helper functions grouped in clearly labeled local sections or nested `#[cfg(test)]` modules so these already-large files do not become harder to navigate.
- Final verification must use the repo-standard sequence: `cargo fmt -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace --all-features --locked --no-fail-fast`.

## File Map

| Action           | Path                                                      | Responsibility                                                                                                    |
|------------------|-----------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|
| Modify           | `Cargo.toml`                                              | Pin `rig-core 0.35.0` and `graph-flow 0.5.1` in workspace dependencies                                            |
| Modify           | `Cargo.lock`                                              | Record the resolved dependency graph after the version bump                                                       |
| Modify           | `crates/scorpio-core/src/providers/mod.rs`                | Add `ProviderId::DeepSeek` and its public string/env-hint surface                                                 |
| Modify           | `crates/scorpio-core/src/config.rs`                       | Accept `deepseek`, add `[providers.deepseek]`, load `SCORPIO_DEEPSEEK_API_KEY`, count the key in readiness checks |
| Modify           | `crates/scorpio-core/src/settings.rs`                     | Persist `deepseek_api_key` in the user config boundary with redaction                                             |
| Modify           | `crates/scorpio-core/src/rate_limit.rs`                   | Add DeepSeek RPM registration to `ProviderRateLimiters`                                                           |
| Modify           | `crates/scorpio-core/src/providers/factory/client.rs`     | Construct native `rig::providers::deepseek::Client` handles and validate `deepseek` provider IDs                  |
| Modify           | `crates/scorpio-core/src/providers/factory/agent.rs`      | Add DeepSeek agent dispatch and adapt mutable chat-history handling for `rig-core 0.35.0`                         |
| Modify           | `crates/scorpio-core/src/agents/analyst/equity/common.rs` | Lock DeepSeek onto the existing typed analyst path with focused regression coverage                               |
| Modify if needed | `crates/scorpio-core/src/workflow/`                       | Apply only directly affected `graph-flow 0.5.1` compatibility fixes proven by the smoke slices                    |
| Modify           | `crates/scorpio-cli/src/cli/setup/steps.rs`               | Surface DeepSeek in setup key collection, routing eligibility, and wizard helper tests                            |
| Modify           | `.env.example`                                            | Document `SCORPIO_DEEPSEEK_API_KEY`                                                                               |
| Modify           | `README.md`                                               | Document DeepSeek as a supported provider in the public setup path                                                |

## Chunk 0: Local Prerequisite

### Task 0: Confirm `protoc` is available before any cargo build step

**Files:**
- None

- [ ] **Step 1: Verify the protobuf compiler is installed**

Run: `protoc --version`

Expected: prints a version string. If the command is missing, install it with `brew install protobuf` on macOS (or the platform-equivalent setup from `AGENTS.md`) before continuing.

## Chunk 1: Dependency Pins and Core DeepSeek Plumbing

### Task 1: Upgrade dependencies and add native DeepSeek provider support

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/scorpio-core/src/providers/mod.rs`
- Modify: `crates/scorpio-core/src/config.rs`
- Modify: `crates/scorpio-core/src/settings.rs`
- Modify: `crates/scorpio-core/src/rate_limit.rs`
- Modify: `crates/scorpio-core/src/providers/factory/client.rs`

- [ ] **Step 1: Write failing provider-identity and config-acceptance tests**

Add these exact tests before touching production code:

```rust
// crates/scorpio-core/src/providers/mod.rs
#[test]
fn provider_id_deepseek_exposes_strings_and_missing_key_hint() {
    assert_eq!(ProviderId::DeepSeek.as_str(), "deepseek");
    assert_eq!(ProviderId::DeepSeek.to_string(), "deepseek");
    assert_eq!(ProviderId::DeepSeek.missing_key_hint(), "SCORPIO_DEEPSEEK_API_KEY");
}

// crates/scorpio-core/src/config.rs
#[test]
fn deserialize_provider_name_accepts_deepseek() {
    let result = deserialize_provider_name(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new("deepseek"),
    );
    assert_eq!(result.unwrap(), "deepseek");
}

#[test]
fn deserialize_provider_name_unknown_lists_deepseek() {
    let err = deserialize_provider_name(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new("badprovider"),
    )
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
fn missing_llm_key_warning_mentions_deepseek() {
    assert!(missing_llm_key_warning().contains("SCORPIO_DEEPSEEK_API_KEY"));
}

#[test]
fn config_without_providers_deepseek_still_deserializes() {
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
    let cfg = Config::load_from(&path).expect("config should load without [providers.deepseek]");
    assert_eq!(cfg.providers.deepseek.rpm, default_deepseek_settings().rpm);
    assert!(cfg.providers.deepseek.api_key.is_none());
}

#[test]
fn load_from_user_path_env_deepseek_api_key_overrides_partial_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let partial = crate::settings::PartialConfig {
        deepseek_api_key: Some("deepseek-file-key".into()),
        ..Default::default()
    };

    crate::settings::save_user_config_at(&partial, &path).expect("save partial config");
    unsafe {
        std::env::set_var("SCORPIO_DEEPSEEK_API_KEY", "deepseek-env-key");
    }
    let result = Config::load_from_user_path(&path);
    unsafe {
        std::env::remove_var("SCORPIO_DEEPSEEK_API_KEY");
    }
    let cfg = result.expect("env key should override persisted partial config key");

    assert_eq!(
        cfg.providers
            .deepseek
            .api_key
            .as_ref()
            .map(ExposeSecret::expose_secret),
        Some("deepseek-env-key")
    );
}

#[test]
fn env_override_deepseek_rpm_wins_over_file_value() {
    let _guard = ENV_LOCK.lock().unwrap();
    let toml = format!("{MINIMAL_CONFIG_TOML}\n\n[providers.deepseek]\nrpm = 12\n");
    let (_dir, path) = write_config(&toml);
    unsafe {
        std::env::set_var("SCORPIO__PROVIDERS__DEEPSEEK__RPM", "45");
    }
    let result = Config::load_from(&path);
    unsafe {
        std::env::remove_var("SCORPIO__PROVIDERS__DEEPSEEK__RPM");
    }
    let cfg = result.expect("env rpm should override file-backed deepseek rpm");
    assert_eq!(cfg.providers.deepseek.rpm, 45);
}
```

- [ ] **Step 2: Write failing persisted-settings, rate-limit, and factory tests**

Add these tests before production changes:

```rust
// crates/scorpio-core/src/settings.rs
#[test]
fn roundtrip_full_config_preserves_deepseek_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = full_partial_config();

    save_user_config_at(&original, &path).expect("save should succeed");
    let loaded = load_user_config_at(&path).expect("load should succeed");

    assert_eq!(loaded.deepseek_api_key.as_deref(), Some("deepseek-key"));
    assert_eq!(loaded, original);
}

#[test]
fn debug_redacts_deepseek_api_key() {
    let cfg = PartialConfig {
        deepseek_api_key: Some("sk-deepseek-secret".into()),
        ..Default::default()
    };
    let output = format!("{cfg:?}");
    assert!(!output.contains("sk-deepseek-secret"));
    assert!(output.contains("[REDACTED]"));
}

// crates/scorpio-core/src/rate_limit.rs
#[test]
fn provider_rate_limiters_construction_includes_deepseek() {
    let cfg = providers_config_with(&[(ProviderId::DeepSeek, 75)]);
    let registry = ProviderRateLimiters::from_config(&cfg);
    assert!(registry.get(ProviderId::DeepSeek).is_some());
    assert_eq!(registry.get(ProviderId::DeepSeek).map(|l| l.label()), Some("deepseek"));
}

#[test]
fn provider_rate_limiters_zero_rpm_disables_deepseek() {
    let cfg = providers_config_with(&[(ProviderId::DeepSeek, 0)]);
    let registry = ProviderRateLimiters::from_config(&cfg);
    assert!(registry.get(ProviderId::DeepSeek).is_none());
}

// crates/scorpio-core/src/providers/factory/client.rs
#[test]
fn validate_provider_id_deepseek_returns_deepseek() {
    let result = validate_provider_id("deepseek");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), ProviderId::DeepSeek);
}

#[test]
fn factory_missing_deepseek_key_returns_config_error() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let result = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &ProvidersConfig::default(),
        &ProviderRateLimiters::default(),
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("SCORPIO_DEEPSEEK_API_KEY"), "expected env var hint in: {msg}");
}

#[test]
fn factory_creates_deepseek_client() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let handle = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &providers_config_with_deepseek(),
        &ProviderRateLimiters::default(),
    )
    .unwrap();

    assert_eq!(handle.provider_name(), "deepseek");
    assert_eq!(handle.model_id(), "deepseek-chat");
    assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
}

#[test]
fn create_completion_model_attaches_deepseek_rate_limiter() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let providers = ProvidersConfig {
        deepseek: ProviderSettings {
            api_key: Some(SecretString::from("test-deepseek-key")),
            base_url: None,
            rpm: 75,
        },
        ..ProvidersConfig::default()
    };
    let limiters = ProviderRateLimiters::from_config(&providers);

    let handle = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &providers,
        &limiters,
    )
    .unwrap();

    assert_eq!(handle.rate_limiter().map(|l| l.label()), Some("deepseek"));
}

#[test]
fn factory_creates_deepseek_client_with_base_url_override() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let providers = ProvidersConfig {
        deepseek: ProviderSettings {
            api_key: Some(SecretString::from("test-deepseek-key")),
            base_url: Some("https://deepseek.example.com/v1".to_owned()),
            rpm: 60,
        },
        ..ProvidersConfig::default()
    };

    let handle = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &providers,
        &ProviderRateLimiters::default(),
    )
    .unwrap();

    assert_eq!(handle.provider_name(), "deepseek");
    assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
}

#[test]
fn factory_invalid_deepseek_base_url_returns_config_error() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let providers = ProvidersConfig {
        deepseek: ProviderSettings {
            api_key: Some(SecretString::from("test-deepseek-key")),
            base_url: Some("://invalid-url".to_owned()),
            rpm: 60,
        },
        ..ProvidersConfig::default()
    };

    let result = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &providers,
        &ProviderRateLimiters::default(),
    );

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("failed to create DeepSeek client with base_url"));
}

#[test]
fn factory_creates_deepseek_client_for_deep_thinking_tier() {
    let mut cfg = sample_llm_config();
    cfg.deep_thinking_provider = "deepseek".to_owned();
    cfg.deep_thinking_model = "deepseek-reasoner".to_owned();

    let handle = create_completion_model(
        ModelTier::DeepThinking,
        &cfg,
        &providers_config_with_deepseek(),
        &ProviderRateLimiters::default(),
    )
    .unwrap();

    assert_eq!(handle.provider_name(), "deepseek");
    assert_eq!(handle.model_id(), "deepseek-reasoner");
    assert!(matches!(handle.client, ProviderClient::DeepSeek(_)));
}
```

- [ ] **Step 3: Bump the workspace dependency pins and refresh the lockfile**

Update the workspace dependencies in `Cargo.toml` to:

```toml
[workspace.dependencies]
rig-core = "0.35.0"
graph-flow = { version = "0.5.1", features = ["rig"] }
```

Run: `cargo update -p rig-core --precise 0.35.0 && cargo update -p graph-flow --precise 0.5.1`

Expected: `Cargo.lock` changes and no command-level errors.

- [ ] **Step 4: Run the focused red-state slice**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(provider_id_deepseek_exposes_strings_and_missing_key_hint) | test(deserialize_provider_name_accepts_deepseek) | test(deserialize_provider_name_unknown_lists_deepseek) | test(load_from_reads_deepseek_api_key_from_env) | test(has_any_llm_key_counts_deepseek_key) | test(env_override_supports_deepseek_rate_limit) | test(load_from_user_path_reads_deepseek_api_key_from_partial_config) | test(missing_llm_key_warning_mentions_deepseek) | test(config_without_providers_deepseek_still_deserializes) | test(load_from_user_path_env_deepseek_api_key_overrides_partial_config) | test(env_override_deepseek_rpm_wins_over_file_value) | test(roundtrip_full_config_preserves_deepseek_api_key) | test(debug_redacts_deepseek_api_key) | test(provider_rate_limiters_construction_includes_deepseek) | test(provider_rate_limiters_zero_rpm_disables_deepseek) | test(validate_provider_id_deepseek_returns_deepseek) | test(factory_missing_deepseek_key_returns_config_error) | test(factory_creates_deepseek_client) | test(create_completion_model_attaches_deepseek_rate_limiter) | test(factory_creates_deepseek_client_with_base_url_override) | test(factory_invalid_deepseek_base_url_returns_config_error) | test(factory_creates_deepseek_client_for_deep_thinking_tier)'`

Expected: FAIL with missing `DeepSeek` symbols, non-exhaustive `match` arms, missing config fields, or the known `rig-core 0.35.0` compile break in `crates/scorpio-core/src/providers/factory/agent.rs`.

- [ ] **Step 5: Implement the minimal DeepSeek plumbing across the core seams**

Make these exact shape changes:

```rust
// crates/scorpio-core/src/providers/mod.rs
pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    Copilot,
    OpenRouter,
    DeepSeek,
}

// crates/scorpio-core/src/config.rs
pub struct ProvidersConfig {
    pub openai: ProviderSettings,
    pub anthropic: ProviderSettings,
    pub gemini: ProviderSettings,
    pub copilot: ProviderSettings,
    pub openrouter: ProviderSettings,
    #[serde(default = "default_deepseek_settings")]
    pub deepseek: ProviderSettings,
}

// crates/scorpio-core/src/settings.rs
pub struct PartialConfig {
    pub deepseek_api_key: Option<String>,
    // existing fields unchanged
}

// crates/scorpio-core/src/providers/factory/client.rs
use rig::providers::{anthropic, deepseek, gemini, openai, openrouter};

pub(crate) enum ProviderClient {
    OpenAI(openai::Client),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
    Copilot(CopilotProviderClient),
    OpenRouter(openrouter::Client),
    DeepSeek(deepseek::Client),
}
```

Implementation details to apply in code, not as comments:

- Add `deepseek` to `ProviderId::as_str()` and `missing_key_hint()`.
- Extend `deserialize_provider_name()` and every supported-provider error string to include `deepseek`.
- Add `default_deepseek_settings()` with the same `ProviderSettings` shape as the other keyed HTTP providers.
- Add `#[serde(default = "default_deepseek_settings")]` on `ProvidersConfig.deepseek` so configs without `[providers.deepseek]` keep deserializing.
- Extend `ProvidersConfig::settings_for(...)`, `base_url_for(...)`, and `rpm_for(...)` to return the DeepSeek settings branch.
- Inject `partial.deepseek_api_key` and `SCORPIO_DEEPSEEK_API_KEY` into `cfg.providers.deepseek.api_key`.
- Extract the missing-LLM-key warning into a small production helper or constant, e.g. `missing_llm_key_warning() -> &'static str`, then extend both that helper and `has_any_llm_key()` to count DeepSeek.
- Extend `ProviderRateLimiters::from_config()` and its test helpers with `ProviderId::DeepSeek`.
- Add `providers_config_with_deepseek()` in `client.rs` test helpers.
- Extend `validate_provider_id(...)` so `"deepseek"` resolves to `ProviderId::DeepSeek`.
- In `create_provider_client_for(...)`, construct DeepSeek with:

```rust
ProviderId::DeepSeek => {
    let key = settings.api_key.as_ref().ok_or_else(|| missing_key_error(provider))?;
    let client = match base_url {
        Some(url) => deepseek::Client::builder()
            .api_key(key.expose_secret())
            .base_url(url)
            .build()
            .map_err(|e| config_error(&format!(
                "failed to create DeepSeek client with base_url \"{url}\": {e}"
            )))?,
        None => deepseek::Client::new(key.expose_secret())
            .map_err(|e| config_error(&format!("failed to create DeepSeek client: {e}")))?,
    };
    Ok(ProviderClient::DeepSeek(client))
}
```

- [ ] **Step 6: Run a compile-expectation checkpoint after the plumbing lands**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(provider_id_deepseek_exposes_strings_and_missing_key_hint) | test(deserialize_provider_name_accepts_deepseek) | test(deserialize_provider_name_unknown_lists_deepseek) | test(load_from_reads_deepseek_api_key_from_env) | test(has_any_llm_key_counts_deepseek_key) | test(env_override_supports_deepseek_rate_limit) | test(load_from_user_path_reads_deepseek_api_key_from_partial_config) | test(missing_llm_key_warning_mentions_deepseek) | test(config_without_providers_deepseek_still_deserializes) | test(load_from_user_path_env_deepseek_api_key_overrides_partial_config) | test(env_override_deepseek_rpm_wins_over_file_value) | test(roundtrip_full_config_preserves_deepseek_api_key) | test(debug_redacts_deepseek_api_key) | test(provider_rate_limiters_construction_includes_deepseek) | test(provider_rate_limiters_zero_rpm_disables_deepseek) | test(validate_provider_id_deepseek_returns_deepseek) | test(factory_missing_deepseek_key_returns_config_error) | test(factory_creates_deepseek_client) | test(create_completion_model_attaches_deepseek_rate_limiter) | test(factory_creates_deepseek_client_with_base_url_override) | test(factory_invalid_deepseek_base_url_returns_config_error) | test(factory_creates_deepseek_client_for_deep_thinking_tier)'`

Expected: still may FAIL on the known `rig-core 0.35.0` wrapper compile break in `crates/scorpio-core/src/providers/factory/agent.rs`. If it fails only there, stop and continue with Chunk 2. If it fails in the files touched by Chunk 1, fix those Chunk 1 regressions before moving on. If the `graph-flow 0.5.1` bump also exposes compile failures in directly affected workflow call sites, fix only those compatibility points and do not broaden the workflow diff beyond what the targeted smoke slices justify.

- [ ] **Step 7: Carry the dependency/provider slice forward uncommitted into Chunk 2**

Expected: do not commit yet. The first commit for this migration happens only after Chunk 2 restores a green state and `cargo check --workspace --all-targets --locked` passes.

## Chunk 2: Agent Wrapper Migration for `rig-core 0.35.0`

### Task 2: Localize the chat-history migration and lock DeepSeek onto the typed analyst path

**Files:**
- Modify: `crates/scorpio-core/src/providers/factory/agent.rs`
- Modify: `crates/scorpio-core/src/agents/analyst/equity/common.rs`
- Modify if needed: directly affected files under `crates/scorpio-core/src/workflow/`

- [ ] **Step 1: Write the failing DeepSeek agent, analyst-path, and chat-history tests**

Add these tests before changing production code:

```rust
#[tokio::test]
async fn build_agent_creates_deepseek_agent() {
    let mut cfg = sample_llm_config();
    cfg.quick_thinking_provider = "deepseek".to_owned();
    cfg.quick_thinking_model = "deepseek-chat".to_owned();

    let handle = super::super::client::create_completion_model(
        crate::providers::ModelTier::QuickThinking,
        &cfg,
        &providers_config_with_deepseek(),
        &ProviderRateLimiters::default(),
    )
    .unwrap();

    let agent = build_agent(&handle, "You are a test agent.");
    assert_eq!(agent.provider_name(), "deepseek");
    assert!(matches!(&agent.inner, LlmAgentInner::DeepSeek(_)));
}

#[test]
fn append_response_messages_appends_new_messages_to_existing_history() {
    let mut history = vec![Message::user("prior")];
    let response = PromptResponse::new("ok", rig::completion::Usage::default()).with_messages(vec![
        Message::user("next"),
        Message::assistant("done"),
    ]);

    append_response_messages(&mut history, &response);

    assert_eq!(history.len(), 3);
}

#[test]
fn append_response_messages_is_noop_when_provider_returns_no_messages() {
    let mut history = vec![Message::user("prior")];
    let response = PromptResponse::new("ok", rig::completion::Usage::default());

    append_response_messages(&mut history, &response);

    assert_eq!(history.len(), 1);
}

// crates/scorpio-core/src/agents/analyst/equity/common.rs
#[tokio::test]
async fn run_analyst_inference_uses_typed_path_for_deepseek() {
    use rig::agent::TypedPromptResponse;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
    struct Output {
        value: i32,
    }

    let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
        ProviderId::DeepSeek,
        "deepseek-chat",
        vec![],
        vec![],
    );
    agent.push_typed_ok(TypedPromptResponse::new(
        Output { value: 42 },
        sample_usage(10),
    ));

    let outcome = run_analyst_inference(
        &agent,
        "prompt",
        Duration::from_millis(50),
        &fast_policy(),
        1,
        |_s: &str| -> Result<Output, crate::error::TradingError> {
            unreachable!("parse hook should not be called on DeepSeek typed path")
        },
        |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
    )
    .await
    .unwrap();

    assert_eq!(outcome.output.value, 42);
    assert_eq!(agent_test_support::typed_attempts(&agent), 1);
    assert_eq!(agent_test_support::text_turn_attempts(&agent), 0);
    assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
}
```

Use the real `rig::completion::Message` constructors already present in this file. If the exact constructor helpers differ, adapt the literals but keep the test names and assertions.

- [ ] **Step 2: Run the focused red-state slice**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_agent_creates_deepseek_agent) | test(append_response_messages_appends_new_messages_to_existing_history) | test(append_response_messages_is_noop_when_provider_returns_no_messages) | test(run_analyst_inference_uses_typed_path_for_deepseek)'`

Expected: FAIL because `LlmAgentInner::DeepSeek`, the history helper, or the DeepSeek analyst path are not fully wired yet.

- [ ] **Step 3: Implement the minimal agent dispatch and history adaptation**

Add the new type alias and dispatch variant:

```rust
type DeepSeekModel = rig::providers::deepseek::CompletionModel;

enum LlmAgentInner {
    OpenAI(rig::agent::Agent<OpenAIModel>),
    Anthropic(rig::agent::Agent<AnthropicModel>),
    Gemini(rig::agent::Agent<GeminiModel>),
    Copilot(rig::agent::Agent<CopilotCompletionModel>),
    OpenRouter(rig::agent::Agent<OpenRouterModel>),
    DeepSeek(rig::agent::Agent<DeepSeekModel>),
    #[cfg(test)]
    Mock(MockLlmAgent),
}
```

Then make these behavior changes and nothing broader:

1. Extend `dispatch_llm_agent!` and `build_agent_inner(...)` with the DeepSeek branch.
2. Replace the old mutable-history borrow path in `chat_details(...)` with a cloned-history request plus an append helper:

```rust
fn append_response_messages(chat_history: &mut Vec<Message>, response: &PromptResponse) {
    if let Some(messages) = &response.messages {
        chat_history.extend(messages.clone());
    }
}

pub async fn chat_details(
    &self,
    prompt: &str,
    chat_history: &mut Vec<Message>,
) -> Result<PromptResponse, PromptError> {
    use rig::agent::PromptRequest;

    let response = dispatch_llm_agent!(
        &self.inner,
        |agent| {
            PromptRequest::from_agent(agent, prompt)
                .with_history(chat_history.clone())
                .extended_details()
                .await
        },
        mock = |agent| agent.chat_details(prompt, chat_history).await
    )?;

    append_response_messages(chat_history, &response);
    Ok(response)
}
```

3. Keep `ProviderId::DeepSeek` on `run_analyst_inference(...)`'s native typed-output path. Do not add an OpenRouter-style text fallback or a Gemini-specific schema-violation fallback for DeepSeek unless the new regression proves that Scorpio needs one.

Rationale: upstream `rig-core 0.35.0` returns `PromptResponse::with_messages(new_messages)`, so `append_response_messages(...)` should append the provider delta rather than rebuild the full transcript.

Do **not** rewrite `LlmAgent::chat(...)` if the direct immutable `agent.chat(prompt, chat_history).await` path still compiles under `rig-core 0.35.0`. Keep the smaller correct change.

- [ ] **Step 4: Re-run the focused agent slice plus one retry regression**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(build_agent_creates_deepseek_agent) | test(append_response_messages_appends_new_messages_to_existing_history) | test(append_response_messages_is_noop_when_provider_returns_no_messages) | test(run_analyst_inference_uses_typed_path_for_deepseek) | test(chat_with_retry_details_retries_and_truncates_partial_history)'`

Expected: PASS.

- [ ] **Step 5: Re-run the full focused DeepSeek slice after the wrapper fix**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(provider_id_deepseek_exposes_strings_and_missing_key_hint) | test(deserialize_provider_name_accepts_deepseek) | test(deserialize_provider_name_unknown_lists_deepseek) | test(load_from_reads_deepseek_api_key_from_env) | test(has_any_llm_key_counts_deepseek_key) | test(env_override_supports_deepseek_rate_limit) | test(load_from_user_path_reads_deepseek_api_key_from_partial_config) | test(missing_llm_key_warning_mentions_deepseek) | test(config_without_providers_deepseek_still_deserializes) | test(load_from_user_path_env_deepseek_api_key_overrides_partial_config) | test(env_override_deepseek_rpm_wins_over_file_value) | test(roundtrip_full_config_preserves_deepseek_api_key) | test(debug_redacts_deepseek_api_key) | test(provider_rate_limiters_construction_includes_deepseek) | test(provider_rate_limiters_zero_rpm_disables_deepseek) | test(validate_provider_id_deepseek_returns_deepseek) | test(factory_missing_deepseek_key_returns_config_error) | test(factory_creates_deepseek_client) | test(create_completion_model_attaches_deepseek_rate_limiter) | test(factory_creates_deepseek_client_with_base_url_override) | test(factory_invalid_deepseek_base_url_returns_config_error) | test(factory_creates_deepseek_client_for_deep_thinking_tier) | test(build_agent_creates_deepseek_agent) | test(append_response_messages_appends_new_messages_to_existing_history) | test(append_response_messages_is_noop_when_provider_returns_no_messages) | test(run_analyst_inference_uses_typed_path_for_deepseek) | test(chat_with_retry_details_retries_and_truncates_partial_history)'`

Expected: PASS.

- [ ] **Step 6: Run the deferred `graph-flow` smoke slice after the wrapper compile fix**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(pipeline_build_graph_produces_graph_without_panic) | test(pipeline_graph_topology_has_correct_start_and_all_nodes)'`

Expected: PASS. If it fails, fix only the directly affected `graph-flow` integration points; do not broaden the workflow diff beyond what the patch upgrade requires.

- [ ] **Step 7: Run one execution-path `graph-flow` smoke after the structure smoke**

Run: `cargo nextest run -p scorpio-core --all-features --locked -E 'test(run_analysis_cycle_success_path_populates_all_phases)'`

Expected: PASS. If it fails, fix only the directly affected `graph-flow` runtime integration points under `crates/scorpio-core/src/workflow/` and the corresponding targeted tests.

- [ ] **Step 8: Run a workspace-wide compile gate before the first commit**

Run: `cargo check --workspace --all-targets --locked`

Expected: PASS.

- [ ] **Step 9: Commit the green dependency/provider/wrapper slice**

Run: `git add Cargo.toml Cargo.lock crates/scorpio-core/src/providers/mod.rs crates/scorpio-core/src/config.rs crates/scorpio-core/src/settings.rs crates/scorpio-core/src/rate_limit.rs crates/scorpio-core/src/providers/factory/client.rs crates/scorpio-core/src/providers/factory/agent.rs crates/scorpio-core/src/agents/analyst/equity/common.rs && git commit -m "feat(core): add deepseek provider and upgrade runtime deps"`

If Steps 6-7 required direct `graph-flow` compatibility fixes, stage those exact touched `crates/scorpio-core/src/workflow/` files in the same commit.

Expected: one green commit containing the dependency pins, DeepSeek provider/runtime wiring, DeepSeek analyst-path coverage, the `rig-core` wrapper migration, and any direct `graph-flow` compatibility fixes proven by the smoke slices.

## Chunk 3: Setup, Docs, and Final Verification

### Task 3: Surface DeepSeek in the setup wizard

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs`

- [ ] **Step 1: Write the failing setup-wizard tests**

Add these tests before changing production code:

```rust
#[test]
fn validate_step3_result_deepseek_key_returns_ok() {
    let p = PartialConfig {
        deepseek_api_key: Some("sk-deepseek".into()),
        ..Default::default()
    };
    assert!(validate_step3_result(&p).is_ok());
}

#[test]
fn provider_key_and_set_provider_key_handle_deepseek() {
    let mut partial = PartialConfig::default();
    set_provider_key(&mut partial, ProviderId::DeepSeek, Some("sk-deepseek".into()));
    assert_eq!(provider_key(&partial, ProviderId::DeepSeek), Some("sk-deepseek"));
}

#[test]
fn providers_with_keys_includes_deepseek_in_declaration_order() {
    let p = PartialConfig {
        openai_api_key: Some("o".into()),
        deepseek_api_key: Some("d".into()),
        ..Default::default()
    };
    assert_eq!(providers_with_keys(&p), vec![ProviderId::OpenAI, ProviderId::DeepSeek]);
}
```

- [ ] **Step 2: Run the focused red-state slice**

Run: `cargo nextest run -p scorpio-cli --all-features --locked -E 'test(validate_step3_result_deepseek_key_returns_ok) | test(provider_key_and_set_provider_key_handle_deepseek) | test(providers_with_keys_includes_deepseek_in_declaration_order)'`

Expected: FAIL because `PartialConfig` and wizard helpers do not handle DeepSeek yet.

- [ ] **Step 3: Implement the minimal wizard changes**

Make these exact edits:

```rust
pub const WIZARD_PROVIDERS: &[ProviderId] = &[
    ProviderId::OpenAI,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenRouter,
    ProviderId::DeepSeek,
];

fn provider_key(partial: &PartialConfig, provider: ProviderId) -> Option<&str> {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key.as_deref(),
        ProviderId::Anthropic => partial.anthropic_api_key.as_deref(),
        ProviderId::Gemini => partial.gemini_api_key.as_deref(),
        ProviderId::OpenRouter => partial.openrouter_api_key.as_deref(),
        ProviderId::DeepSeek => partial.deepseek_api_key.as_deref(),
        ProviderId::Copilot => None,
    }
}
```

Also extend `set_provider_key(...)`, `validate_step3_result(...)`, and any existing all-providers tests to count DeepSeek.

- [ ] **Step 4: Re-run the focused setup-wizard slice**

Run: `cargo nextest run -p scorpio-cli --all-features --locked -E 'test(validate_step3_result_deepseek_key_returns_ok) | test(provider_key_and_set_provider_key_handle_deepseek) | test(providers_with_keys_includes_deepseek_in_declaration_order) | test(provider_id_display_matches_as_str)'`

Expected: PASS.

- [ ] **Step 5: Commit the wizard support**

Run: `git add crates/scorpio-cli/src/cli/setup/steps.rs && git commit -m "feat(cli): add deepseek to setup wizard"`

Expected: one green commit that surfaces DeepSeek in the interactive setup flow.

### Task 4: Update public docs and examples

**Files:**
- Modify: `.env.example`
- Modify: `README.md`

- [ ] **Step 1: Update `.env.example` with the new provider key**

Add this exact line under the existing LLM provider keys:

```env
SCORPIO_DEEPSEEK_API_KEY=
```

- [ ] **Step 2: Update the public README provider lists and setup note**

Make these exact documentation edits:

- Add `SCORPIO_DEEPSEEK_API_KEY=your-deepseek-key-here` to the env block in the setup section.
- Update any provider lists that currently enumerate `OpenAI`, `Anthropic`, `Gemini`, and `OpenRouter` so they also mention `DeepSeek` where that list is describing supported LLM providers.
- Update the Copilot quick-thinking note so it reads as a positive supported-provider list that includes DeepSeek, e.g. “use OpenAI, Anthropic, Gemini, or DeepSeek for the `quick_thinking_provider`”.

- [ ] **Step 3: Verify the docs mention DeepSeek in both public entry points**

Run: `rg -n "SCORPIO_DEEPSEEK_API_KEY|DeepSeek|deepseek" README.md .env.example`

Expected: matches in both files, with no stale “OpenAI, Anthropic, or Gemini” quick-thinking list left behind.

- [ ] **Step 4: Commit the docs updates**

Run: `git add README.md .env.example && git commit -m "docs(setup): document deepseek provider support"`

Expected: one docs-only commit.

### Task 5: Run the repo verification gate

**Files:**
- Modify: only files required to fix any verification failures from the earlier tasks

- [ ] **Step 1: Run formatting verification**

Run: `cargo fmt -- --check`

Expected: PASS.

- [ ] **Step 2: Run clippy verification**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS.

- [ ] **Step 3: Run full workspace tests with nextest**

Run: `cargo nextest run --workspace --all-features --locked --no-fail-fast`

Expected: PASS.

- [ ] **Step 4: If verification fixes were needed, commit them separately**

Run: `git add <exact files fixed during verification> && git commit -m "fix(core): address deepseek upgrade verification issues"`

Expected: skip this step if Steps 1-3 are already green with no follow-up edits.

- [ ] **Step 5: Confirm the worktree is clean**

Run: `git status --short`

Expected: no output.
