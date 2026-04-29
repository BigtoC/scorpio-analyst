# Setup Model Selection and Rig-Core 0.36.0 Implementation Plan

> **For agentic workers:** REQUIRED: Use `@superpowers:subagent-driven-development` (if subagents available) or `@superpowers:executing-plans` to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `rig-core` to `0.36.0`, remove Scorpio's custom Copilot implementation, and replace manual-only setup step 4 model entry with provider-backed model selection plus manual fallback.

**Architecture:** Land the `rig-core 0.36.0` bump and custom-Copilot removal first so the workspace compiles cleanly against the new upstream API. Then add a focused `scorpio-core` model-discovery module plus a small setup-only provider-settings merge helper, and keep the CLI flow provider-first by moving the new step-4 logic into a dedicated `setup/model_selection.rs` helper module instead of growing `steps.rs` further.

**Tech Stack:** Rust 2024, Cargo workspace, `rig-core 0.36.0`, `tokio`, `futures`, `serde`, `inquire`, `cargo nextest`, `cargo fmt`, `cargo clippy`.

---

**Spec:** `docs/superpowers/specs/2026-04-29-setup-model-selection-design.md`
**Worktree:** Execute from `feature/ehance-model-selection`. Confirm with `git worktree list` before starting.

## Guardrails

- Use `@superpowers:subagent-driven-development` for execution and `@superpowers:verification-before-completion` before declaring the work done.
- CI uses `cargo nextest`, not `cargo test`. Use `cargo nextest` for every targeted test run in this plan.
- Confirm `protoc --version` prints a version string before the first workspace build. If it is missing on macOS, install it with `brew install protobuf`.
- Start with the `rig-core 0.36.0` bump only, gated by Task 1 Step 0's feasibility spike. If `cargo check --workspace --all-features --locked` after the bump fails with errors pointing into `graph-flow` rather than into Scorpio source, stop, revert with `git checkout HEAD -- Cargo.toml Cargo.lock`, hand off the `graph-flow` patch upstream, and re-open this plan after a compatible `graph-flow` release ships. Do not partially execute Task 1 and leave the workspace broken on the feature branch. Note: the workspace currently pins `rig-core 0.35.0` (not `0.32`); the actual delta is 0.35 → 0.36, and `graph-flow 0.5.1`'s manifest declares `rig-core = "0.35.0"`, which Cargo resolves as `>=0.35.0,<0.36.0` and will not unify with `0.36.0`.
- Do not preserve the custom ACP Copilot path behind feature flags, compatibility wrappers, or dead code. Remove it completely.
- Do not add official `rig` Copilot support in this plan. That is the next task.
- Keep `openrouter` manual-only in setup even if upstream adds listing support.
- Keep `providers_with_keys(partial)` as the provider-eligibility gate for step 4.
- Treat `WIZARD_PROVIDERS` order as the deterministic order for “first eligible provider”: `openai`, `anthropic`, `gemini`, `openrouter`, `deepseek`.
- Preserve returned model IDs as-is. If a saved model appears multiple times, move only the first matching occurrence to the front and leave the rest in upstream order.

## File Map

| Action | Path                                                      | Responsibility                                                                                                                      |
|--------|-----------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------|
| Modify | `Cargo.toml`                                              | Pin `rig-core = 0.36.0` in workspace dependencies                                                                                   |
| Modify | `Cargo.lock`                                              | Record the resolved dependency graph after the `rig-core` upgrade                                                                   |
| Delete | `crates/scorpio-core/src/providers/acp.rs`                | Remove the ACP transport that only existed for the custom Copilot provider                                                          |
| Delete | `crates/scorpio-core/src/providers/copilot.rs`            | Remove the custom Copilot completion model that depends on removed upstream API surface                                             |
| Modify | `crates/scorpio-core/src/providers/mod.rs`                | Remove `ProviderId::Copilot` and the Copilot module export                                                                          |
| Modify | `crates/scorpio-core/src/config.rs`                       | Remove Copilot from validated provider names, remove `ProvidersConfig.copilot`, and add a setup-safe provider-settings merge helper |
| Modify | `crates/scorpio-core/src/settings.rs`                     | Keep stale provider/model strings round-trippable through `PartialConfig` and add recovery-path coverage                            |
| Modify | `crates/scorpio-core/src/rate_limit.rs`                   | Remove Copilot limiter wiring and keep the remaining provider registry correct                                                      |
| Modify | `crates/scorpio-core/src/providers/factory/mod.rs`        | Remove Copilot preflight export and add discovery-module exports                                                                    |
| Modify | `crates/scorpio-core/src/providers/factory/client.rs`     | Remove Copilot client construction and provider validation branches                                                                 |
| Modify | `crates/scorpio-core/src/app/mod.rs`                      | Remove the runtime Copilot preflight call so analysis startup matches the new provider surface                                      |
| Create | `crates/scorpio-core/src/providers/factory/discovery.rs`  | Own provider-backed model discovery and normalized setup outcomes                                                                   |
| Modify | `crates/scorpio-core/src/providers/factory/agent.rs`      | Remove Copilot dispatch and keep remaining provider agent construction intact                                                       |
| Modify | `crates/scorpio-core/src/agents/analyst/equity/common.rs` | Remove stale Copilot references from typed-path comments and related assertions                                                     |
| Modify | `crates/scorpio-cli/src/cli/setup/mod.rs`                 | Register a new `model_selection` helper module                                                                                      |
| Create | `crates/scorpio-cli/src/cli/setup/model_selection.rs`     | Hold step-4-specific discovery bootstrap, model option ordering, and manual-fallback logic                                          |
| Modify | `crates/scorpio-cli/src/cli/setup/steps.rs`               | Delegate step 4 to the new helper module and remove Copilot-only health-check branches                                              |
| Modify | `README.md`                                               | Document the new step-4 model-selection UX and temporary Copilot unavailability                                                     |

## Chunk 1: Rig-Core 0.36.0 and Custom Copilot Removal

### Task 1: Upgrade `rig-core` and remove Copilot from the validated provider surface

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/scorpio-core/src/providers/mod.rs`
- Modify: `crates/scorpio-core/src/config.rs`
- Modify: `crates/scorpio-core/src/settings.rs`
- Modify: `crates/scorpio-core/src/rate_limit.rs`

- [ ] **Step 0: Run the graph-flow feasibility spike before any other code changes**

Verify the workspace can compile against `rig-core 0.36.0` end-to-end before touching any other code. This guards against the `graph-flow 0.5.1` × `rig-core 0.36.0` incompatibility that would otherwise be discovered only after Tasks 1-2 commit destructive Copilot deletions:

```bash
cargo update -p rig-core --precise 0.36.0
cargo check --workspace --all-features --locked
```

Expected outcomes:

- `cargo check` succeeds. Continue.
- `cargo check` fails with errors pointing only into Scorpio source (uses of `ProviderId::Copilot`, `ProviderClient::Copilot`, `preflight_copilot_if_configured`, etc.). These are exactly the cleanups Tasks 1-2 will perform. Continue.
- `cargo check` fails with errors pointing into `graph-flow` (e.g. trait-bound mismatches inside `graph_flow::context` or `graph_flow::executor`, or duplicate-version errors for `rig-core` re-exports). STOP. Revert with `git checkout HEAD -- Cargo.toml Cargo.lock` and abandon this plan until a compatible `graph-flow` release ships. Do not proceed to Step 1.

This step intentionally leaves `Cargo.toml`/`Cargo.lock` modified on disk if the spike succeeds; Step 3 below will rewrite the same files in a more deliberate form.

- [ ] **Step 1: Write the failing config and recovery tests**

Add these tests before changing production code:

```rust
// crates/scorpio-core/src/config.rs
#[test]
fn deserialize_provider_name_rejects_copilot() {
    let result = deserialize_provider_name(
        serde::de::value::StrDeserializer::<serde::de::value::Error>::new("copilot"),
    );
    let err = result.expect_err("copilot should no longer be accepted");
    let msg = err.to_string();
    assert!(msg.contains("copilot"));
    assert!(msg.contains("openrouter"));
    assert!(msg.contains("deepseek"));
    assert!(!msg.contains("supported: openai, anthropic, gemini, copilot"));
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
    assert!(err.to_string().contains("copilot"));
}

// crates/scorpio-core/src/settings.rs
#[test]
fn load_user_config_at_preserves_stale_copilot_routing_strings() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_toml(
        &dir,
        r#"
quick_thinking_provider = "copilot"
quick_thinking_model = "claude-haiku"
deep_thinking_provider = "openai"
deep_thinking_model = "o3"
"#,
    );

    let loaded = load_user_config_at(&path).expect("partial config should still load");
    assert_eq!(loaded.quick_thinking_provider.as_deref(), Some("copilot"));
    assert_eq!(loaded.quick_thinking_model.as_deref(), Some("claude-haiku"));
    assert_eq!(loaded.deep_thinking_provider.as_deref(), Some("openai"));
}

// crates/scorpio-core/src/config.rs
#[test]
fn load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_dir, path) = write_config(
        r#"
[llm]
quick_thinking_provider = "copilot"
deep_thinking_provider = "openai"
quick_thinking_model = "claude-haiku"
deep_thinking_model = "o3"
"#,
    );

    let err = Config::load_from_user_path(&path)
        .expect_err("a config that still routes to copilot should fail to load at runtime");
    let msg = err.to_string();
    assert!(msg.contains("Copilot"), "expected friendly Copilot reference; got: {msg}");
    assert!(msg.contains("scorpio setup"), "expected guidance to run setup; got: {msg}");
}
```

- [ ] **Step 2: Run the focused tests to verify they fail**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(deserialize_provider_name_rejects_copilot) | test(load_from_rejects_copilot_provider_name) | test(load_user_config_at_preserves_stale_copilot_routing_strings) | test(load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot)'
```

Expected:

- `deserialize_provider_name_rejects_copilot` fails because `copilot` is still accepted.
- `load_from_rejects_copilot_provider_name` fails because config loading still allows `copilot`.
- `load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot` fails because the recovery wrapper around `Config::load_from_user_path` does not exist yet.
- The settings regression may already pass; keep it as the recovery-path guard.

- [ ] **Step 3: Bump the workspace dependency and refresh the lockfile**

Run:

```bash
cargo update -p rig-core --precise 0.36.0
```

Then update `Cargo.toml`:

```toml
[workspace.dependencies]
rig-core = "0.36.0"
```

Expected:

- `Cargo.lock` now resolves `rig-core 0.36.0`.
- Do not touch `graph-flow` here.

- [ ] **Step 4: Implement the validated-provider and rate-limit cleanup**

Make the minimal production edits needed for the tests and new dependency baseline:

```rust
// crates/scorpio-core/src/providers/mod.rs
pub mod factory;

pub enum ProviderId {
    OpenAI,
    Anthropic,
    Gemini,
    OpenRouter,
    DeepSeek,
}

// crates/scorpio-core/src/config.rs
match canonical.as_str() {
    "openai" | "anthropic" | "gemini" | "openrouter" | "deepseek" => Ok(canonical),
    unknown => Err(serde::de::Error::custom(format!(
        "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek)"
    ))),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProvidersConfig {
    pub openai: ProviderSettings,
    pub anthropic: ProviderSettings,
    pub gemini: ProviderSettings,
    pub openrouter: ProviderSettings,
    pub deepseek: ProviderSettings,
}

// crates/scorpio-core/src/rate_limit.rs
let provider_rpms = [
    (ProviderId::OpenAI, cfg.openai.rpm, "openai"),
    (ProviderId::Anthropic, cfg.anthropic.rpm, "anthropic"),
    (ProviderId::Gemini, cfg.gemini.rpm, "gemini"),
    (ProviderId::OpenRouter, cfg.openrouter.rpm, "openrouter"),
    (ProviderId::DeepSeek, cfg.deepseek.rpm, "deepseek"),
];
```

Also update any loops, helper matches, and assertions in these files that still enumerate Copilot.

Add a stale-config recovery wrapper to `Config::load_from_user_path` so analysis startup degrades helpfully when an existing `~/.scorpio-analyst/config.toml` still routes to Copilot. Detect the specific deserialize error and surface a friendlier message; do not silently rewrite the on-disk file. Two implementation details are load-bearing:

1. Rename the existing public `Config::load_from_user_path` body to a private `Config::load_from_user_path_inner(...) -> Result<Self>`, then introduce the new public `load_from_user_path` shown below as a thin wrapper. The plan's `load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot` test calls the public name, so the public signature must stay identical.
2. Walk the `anyhow::Error` chain (or use the alternate `{:#}` formatter) — `err.to_string()` only renders the topmost context, and `Config::load_effective_runtime` already attaches `.context("failed to deserialize configuration")?` over the inner serde error. A plain `to_string().contains(...)` matcher cannot fire because the inner serde message never reaches the topmost layer. Walking `err.chain()` is required for the matcher to work.
3. Pin the marker as a `pub(crate) const` shared by `deserialize_provider_name` and the wrapper so a future error-message edit cannot silently break the contract:

```rust
// crates/scorpio-core/src/config.rs

pub(crate) const STALE_COPILOT_PROVIDER_MARKER: &str = "unknown LLM provider: \"copilot\"";

// inside deserialize_provider_name's Err branch:
Err(serde::de::Error::custom(format!(
    "{STALE_COPILOT_PROVIDER_MARKER} (supported: openai, anthropic, gemini, openrouter, deepseek)"
)))

// new public wrapper:
pub fn load_from_user_path(path: impl AsRef<Path>) -> Result<Config> {
    match Self::load_from_user_path_inner(path) {
        Ok(cfg) => Ok(cfg),
        Err(err) if err.chain().any(|cause| cause.to_string().contains(STALE_COPILOT_PROVIDER_MARKER)) => {
            Err(anyhow::anyhow!(
                "Your saved configuration still routes to the Copilot provider, which has been removed. \
                 Run `scorpio setup` to update routing to a supported provider."
            ))
        }
        Err(err) => Err(err),
    }
}
```

What matters: runtime startup paths (`scorpio analyze`) surface a recognisable `Copilot` + `scorpio setup` message rather than a raw `unknown LLM provider` serde error, and the contract is anchored by the shared constant. The `load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot` test pins this end-to-end through `Config::load_effective_runtime`'s anyhow context wrapper.

- [ ] **Step 5: Re-run the targeted tests and stop if `graph-flow` blocks the upgrade**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(deserialize_provider_name_rejects_copilot) | test(load_from_rejects_copilot_provider_name) | test(load_user_config_at_preserves_stale_copilot_routing_strings) | test(load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot) | test(provider_rate_limiters_construction_mixed_rpms) | test(provider_id_deepseek_exposes_strings_and_missing_key_hint)'
```

Expected:

- PASS.
- If failures now point into `graph-flow` compatibility rather than Copilot removal, stop here and hand off the `graph-flow` patch separately before continuing this plan.

- [ ] **Step 6: Hold the dependency and provider-surface cleanup uncommitted**

Do not commit yet. After Task 1's edits, `cargo build --workspace` will fail because Task 2's match-arm cleanups have not landed: `providers/factory/{client.rs,agent.rs}` still reference `ProviderId::Copilot`, and `app/mod.rs` still calls `preflight_copilot_if_configured`. A commit at this point would land an unbuildable revision on the feature branch, breaking `git bisect` and per-commit CI checks.

Tasks 1 and 2 commit together at the end of Task 2 (Step 5) under one message that covers both the dependency bump and the runtime deletion. Leave the changes from Steps 1-5 in the working tree and proceed directly to Task 2.

### Task 2: Delete the custom Copilot runtime and remove factory/setup wiring

**Files:**
- Delete: `crates/scorpio-core/src/providers/acp.rs`
- Delete: `crates/scorpio-core/src/providers/copilot.rs`
- Modify: `crates/scorpio-core/src/providers/mod.rs`
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs`
- Modify: `crates/scorpio-core/src/providers/factory/client.rs`
- Modify: `crates/scorpio-core/src/providers/factory/agent.rs`
- Modify: `crates/scorpio-core/src/app/mod.rs`
- Modify: `crates/scorpio-core/src/agents/analyst/equity/common.rs`
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs`

- [ ] **Step 1: Write the failing factory validation test**

Add this test before editing the factory:

```rust
// crates/scorpio-core/src/providers/factory/client.rs
#[test]
fn validate_provider_id_rejects_copilot() {
    let err = validate_provider_id("copilot").expect_err("copilot should be rejected");
    let msg = err.to_string();
    assert!(msg.contains("copilot"));
    assert!(msg.contains("openrouter"));
    assert!(msg.contains("deepseek"));
}
```

- [ ] **Step 2: Run the focused factory slice and verify it fails**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(validate_provider_id_rejects_copilot)'
```

Expected:

- FAIL because `validate_provider_id("copilot")` still returns `Ok(ProviderId::Copilot)`.

- [ ] **Step 3: Remove the custom Copilot runtime and update the remaining provider factory paths**

Apply the minimal deletions and match cleanup:

```rust
// crates/scorpio-core/src/providers/factory/mod.rs
mod agent;
mod client;
mod error;
mod retry;
mod text_retry;

pub use client::{CompletionModelHandle, create_completion_model};

// crates/scorpio-core/src/providers/factory/client.rs
pub(crate) enum ProviderClient {
    OpenAI(openai::Client),
    Anthropic(anthropic::Client),
    Gemini(gemini::Client),
    OpenRouter(openrouter::Client),
    DeepSeek(deepseek::Client),
}

fn validate_provider_id(provider: &str) -> Result<ProviderId, TradingError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "openai" => Ok(ProviderId::OpenAI),
        "anthropic" => Ok(ProviderId::Anthropic),
        "gemini" => Ok(ProviderId::Gemini),
        "openrouter" => Ok(ProviderId::OpenRouter),
        "deepseek" => Ok(ProviderId::DeepSeek),
        unknown => Err(config_error(&format!(
            "unknown LLM provider: \"{unknown}\" (supported: openai, anthropic, gemini, openrouter, deepseek)"
        ))),
    }
}

// crates/scorpio-core/src/providers/factory/agent.rs
enum LlmAgentInner {
    OpenAI(rig::agent::Agent<OpenAIModel>),
    Anthropic(rig::agent::Agent<AnthropicModel>),
    Gemini(rig::agent::Agent<GeminiModel>),
    OpenRouter(rig::agent::Agent<OpenRouterModel>),
    DeepSeek(rig::agent::Agent<DeepSeekModel>),
}

// crates/scorpio-core/src/app/mod.rs
use crate::providers::factory::create_completion_model;

// AnalysisRuntime::new
let rate_limiters = ProviderRateLimiters::from_config(&cfg.providers);
```

Delete the two Copilot-only source files. Then remove:

- `pub mod acp;` and `pub mod copilot;` from `crates/scorpio-core/src/providers/mod.rs`
- the `preflight_copilot_if_configured` export and implementation
- the `preflight_copilot_if_configured` import/call in `crates/scorpio-core/src/app/mod.rs`
- `SCORPIO_COPILOT_CLI_PATH` validation helpers and tests
- `ProviderClient::Copilot` dispatch in `agent.rs`
- Copilot-only setup health-check preflight in `steps.rs`, including the `run_single_health_check_rejects_copilot_provider_that_fails_preflight` test and the `SCORPIO_COPILOT_CLI_PATH` env-handling helpers it relies on
- stale “Copilot” wording in `agents/analyst/equity/common.rs`

- [ ] **Step 4: Re-run targeted core and CLI slices**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(validate_provider_id_rejects_copilot) | test(build_agent_creates_openai_agent) | test(build_agent_creates_openrouter_agent) | test(build_agent_creates_deepseek_agent) | test(run_analysis_cycle_success_path_populates_all_phases)'
cargo nextest run -p scorpio-cli --all-features --locked -E 'test(provider_id_display_matches_as_str) | test(run_single_health_check_requires_same_analysis_readiness_as_analyze)'
if rg -n 'ProviderId::Copilot|copilot::|preflight_copilot_if_configured|providers\.copilot|cfg\.copilot' crates/; then exit 1; fi
```

Expected:

- All three commands PASS (the `rg` grep prints no matches, so the `if` block is skipped and the shell exits 0).
- No compile failures remain for `copilot::`, `ProviderClient::Copilot`, or `preflight_copilot_if_configured`.

- [ ] **Step 5: Commit Tasks 1 and 2 atomically**

Stage every file modified by Tasks 1 + 2 plus the deletions, verify the working tree compiles cleanly, and only then commit. This is the first commit on the branch, so it must build cleanly before being recorded; every later commit must continue to compile against `cargo check`. Run `cargo check` BEFORE `git commit` so an unbuildable tree never lands.

```bash
git add Cargo.toml Cargo.lock crates/scorpio-core/src/providers/mod.rs crates/scorpio-core/src/config.rs crates/scorpio-core/src/settings.rs crates/scorpio-core/src/rate_limit.rs crates/scorpio-core/src/providers/factory/mod.rs crates/scorpio-core/src/providers/factory/client.rs crates/scorpio-core/src/providers/factory/agent.rs crates/scorpio-core/src/app/mod.rs crates/scorpio-core/src/agents/analyst/equity/common.rs crates/scorpio-cli/src/cli/setup/steps.rs
git rm crates/scorpio-core/src/providers/acp.rs crates/scorpio-core/src/providers/copilot.rs
cargo check --workspace --all-features --locked
git commit -m "refactor(providers): bump rig-core to 0.36.0 and remove custom copilot runtime"
```

Expected: `cargo check` exits 0 BEFORE the commit. If it fails, do not commit; iterate on the working tree until `cargo check` passes. Cargo.lock is included in the `git add` list above to keep the committed tree reproducible against the bumped `rig-core 0.36.0` resolution.

## Chunk 2: Core Provider Discovery for Setup

### Task 3: Add a path-aware setup provider-settings loader that ignores stale routing

**Files:**
- Modify: `crates/scorpio-core/src/config.rs`

- [ ] **Step 1: Write the failing helper tests**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `crates/scorpio-core/src/config.rs` so they pick up the existing `ENV_LOCK`, `MINIMAL_CONFIG_TOML`, and `write_config(...)` helpers. Add them before the helper exists:

```rust
// crates/scorpio-core/src/config.rs
#[test]
fn load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing() {
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
        openai_api_key: Some("sk-openai".into()),
        ..Default::default()
    };

    let providers = Config::load_effective_providers_config_from_user_path(&path, &partial)
        .expect("provider settings should load without validating stale routing");

    assert_eq!(
        providers.openai.api_key.as_ref().map(ExposeSecret::expose_secret),
        Some("sk-openai")
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
        std::env::set_var("SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL", "https://deepseek.example.com/v1");
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
```

- [ ] **Step 2: Run the helper slice and verify it fails**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing) | test(load_effective_providers_config_from_user_path_reads_env_base_url_override)'
```

Expected:

- FAIL because `Config::load_effective_providers_config_from_user_path` does not exist yet.

- [ ] **Step 3: Implement the path-aware provider-settings loader in `config.rs`**

Add a dedicated helper instead of reusing `Config::load_effective_runtime`:

```rust
impl Config {
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

        let settings = config::Config::builder()
            .add_source(config::File::from(path.as_ref()).required(false))
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
    inject_provider_env_override(&mut providers.openai.api_key, "SCORPIO_OPENAI_API_KEY", "openai");
    inject_provider_env_override(&mut providers.anthropic.api_key, "SCORPIO_ANTHROPIC_API_KEY", "anthropic");
    inject_provider_env_override(&mut providers.gemini.api_key, "SCORPIO_GEMINI_API_KEY", "gemini");
    inject_provider_env_override(&mut providers.openrouter.api_key, "SCORPIO_OPENROUTER_API_KEY", "openrouter");
    inject_provider_env_override(&mut providers.deepseek.api_key, "SCORPIO_DEEPSEEK_API_KEY", "deepseek");
}

fn inject_provider_env_override(
    field: &mut Option<SecretString>,
    env_var: &str,
    provider_name: &str,
) {
    if let Some(key) = secret_from_env(env_var) {
        if field.is_some() {
            tracing::warn!(provider = provider_name, env_var, "env var overrides user config file secret");
        }
        *field = Some(key);
    }
}
```

Do not deserialize `LlmConfig` inside this helper. That is the entire point of the stale-Copilot recovery path.

Also keep the helper contract explicit in the code comments: it preserves file-backed `[providers.*]` settings plus env overrides and current wizard secrets, but it does not attempt to validate or reuse the current `[llm]` routing values.

- [ ] **Step 4: Re-run the helper slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing) | test(load_effective_providers_config_from_user_path_reads_env_base_url_override)'
```

Expected: PASS.

- [ ] **Step 5: Commit the helper**

Run:

```bash
git add crates/scorpio-core/src/config.rs
git commit -m "refactor(config): add setup-safe provider settings loader"
```

### Task 4: Add provider-backed model discovery in `scorpio-core`

**Files:**
- Modify: `crates/scorpio-core/Cargo.toml`
- Create: `crates/scorpio-core/src/providers/factory/discovery.rs`
- Modify: `crates/scorpio-core/src/providers/factory/mod.rs`

- [ ] **Step 0: Enable `tokio` `test-util` for paused-clock tests**

The new discovery test uses `#[tokio::test(start_paused = true)]`, which requires the `test-util` Cargo feature. The workspace-level `tokio = { version = "1", features = ["full"] }` does **not** include `test-util` (it is intentionally excluded from `full`). Add an override in `crates/scorpio-core/Cargo.toml`'s `[dev-dependencies]` so the feature is enabled only in test builds:

```toml
# crates/scorpio-core/Cargo.toml
[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
# ... existing dev-deps unchanged
```

Without this, the failing test in Step 1 will not compile (the `start_paused` attribute is unknown) and the TDD red-step in Step 2 degrades from "test fails by assertion" to "test fails to compile" (cf. the broader Adversarial finding about compile-vs-assertion FAIL).

- [ ] **Step 1: Write the failing discovery tests in the new module**

Create `crates/scorpio-core/src/providers/factory/discovery.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rig::model::{Model, ModelList};

    #[test]
    fn openrouter_returns_manual_only() {
        let outcome = manual_only_outcome(ProviderId::OpenRouter);
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::ManualOnly {
                reason: "Model listing is manual-only for openrouter; enter the model manually.".into(),
            }
        );
    }

    #[test]
    fn normalize_model_list_preserves_order_and_duplicates() {
        let list = ModelList::new(vec![
            Model::from_id("gpt-4o-mini"),
            Model::from_id("o3"),
            Model::from_id("o3"),
        ]);

        let outcome = normalize_model_list(ProviderId::OpenAI, list);
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::Listed(vec![
                "gpt-4o-mini".into(),
                "o3".into(),
                "o3".into(),
            ])
        );
    }

    #[test]
    fn normalize_empty_model_list_returns_unavailable() {
        let outcome = normalize_model_list(ProviderId::Gemini, ModelList::new(vec![]));
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::Unavailable {
                reason: "No models were returned for gemini; enter the model manually.".into(),
            }
        );
    }

    #[test]
    fn unavailable_reason_is_sanitized() {
        let outcome = unavailable_from_error(
            ProviderId::Anthropic,
            "Bearer sk-ant-secret-token leaked from upstream",
        );

        let ModelDiscoveryOutcome::Unavailable { reason } = outcome else {
            panic!("expected unavailable outcome");
        };

        assert!(reason.contains("anthropic"));
        assert!(!reason.contains("sk-ant-secret-token"));
    }

    #[tokio::test]
    async fn collect_outcomes_keeps_one_result_per_provider() {
        let outcomes = collect_discovery_outcomes(
            [ProviderId::OpenAI, ProviderId::OpenRouter],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => {
                        (provider, ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]))
                    }
                    ProviderId::OpenRouter => (provider, manual_only_outcome(provider)),
                    _ => unreachable!(),
                }
            },
        )
        .await;

        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            outcomes.get(&ProviderId::OpenAI),
            Some(ModelDiscoveryOutcome::Listed(_))
        ));
        assert!(matches!(
            outcomes.get(&ProviderId::OpenRouter),
            Some(ModelDiscoveryOutcome::ManualOnly { .. })
        ));
    }

    #[tokio::test]
    async fn discover_setup_models_with_sanitizes_failures_and_preserves_successes() {
        let outcomes = discover_setup_models_with(
            [ProviderId::OpenAI, ProviderId::Anthropic],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => Ok(ModelList::new(vec![Model::from_id("gpt-4o-mini")])),
                    ProviderId::Anthropic => Err("Bearer sk-ant-secret-token leaked from upstream".to_owned()),
                    _ => unreachable!(),
                }
            },
        )
        .await;

        assert_eq!(
            outcomes.get(&ProviderId::OpenAI),
            Some(&ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]))
        );
        assert_eq!(
            outcomes.get(&ProviderId::Anthropic),
            Some(&ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for anthropic; enter the model manually.".into(),
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn discover_setup_models_with_times_out_slow_providers_without_blocking_others() {
        let outcomes = discover_setup_models_with(
            [ProviderId::OpenAI, ProviderId::Anthropic],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => Ok(ModelList::new(vec![Model::from_id("gpt-4o-mini")])),
                    ProviderId::Anthropic => {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        Ok(ModelList::new(vec![Model::from_id("claude-haiku")]))
                    }
                    _ => unreachable!(),
                }
            },
        )
        .await;

        assert!(matches!(
            outcomes.get(&ProviderId::OpenAI),
            Some(ModelDiscoveryOutcome::Listed(_))
        ));
        let anthropic = outcomes.get(&ProviderId::Anthropic);
        let Some(ModelDiscoveryOutcome::Unavailable { reason }) = anthropic else {
            panic!("expected Unavailable for slow provider; got {anthropic:?}");
        };
        assert!(reason.contains("anthropic"));
        assert!(reason.contains("timed out") || reason.contains("Could not load"));
    }

    #[test]
    fn unavailable_reason_uses_fixed_template_regardless_of_upstream_error_shape() {
        let leak_patterns = [
            "Bearer sk-ant-secret-token leaked from upstream",
            "x-api-key: sk-real-key was rejected",
            "Authorization: Bearer sk-secret-key invalid",
            "request to https://api.example.com?api_key=sk-leaked failed",
            "raw sk-rawtoken at the start of the message",
            "{\"error\":{\"message\":\"Invalid Authorization: Bearer sk-leaked\"}}",
        ];

        for upstream in leak_patterns {
            let outcome = unavailable_from_error(ProviderId::OpenAI, upstream);
            let ModelDiscoveryOutcome::Unavailable { reason } = outcome else {
                panic!("expected unavailable outcome for upstream={upstream:?}");
            };
            assert_eq!(
                reason,
                "Could not load models for openai; enter the model manually.",
                "reason must come from a fixed template; got {reason:?} for upstream={upstream:?}"
            );
            assert!(reason.len() <= 120, "reason exceeds 120-char cap: {reason:?}");
        }
    }
}
```

- [ ] **Step 2: Run the discovery slice and verify it fails**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(openrouter_returns_manual_only) | test(normalize_model_list_preserves_order_and_duplicates) | test(normalize_empty_model_list_returns_unavailable) | test(unavailable_reason_is_sanitized) | test(collect_outcomes_keeps_one_result_per_provider) | test(discover_setup_models_with_sanitizes_failures_and_preserves_successes) | test(discover_setup_models_with_times_out_slow_providers_without_blocking_others) | test(unavailable_reason_uses_fixed_template_regardless_of_upstream_error_shape)'
```

Expected: FAIL because the module and helper functions do not exist yet.

- [ ] **Step 3: Implement the discovery module and export it**

Build the smallest core discovery surface that matches the spec:

```rust
// crates/scorpio-core/src/providers/factory/discovery.rs
use std::collections::HashMap;

use futures::future::join_all;
use rig::client::ModelListingClient;
use rig::model::ModelList;
use rig::providers::{anthropic, deepseek, gemini, openai};

use crate::config::{ProviderSettings, ProvidersConfig};
use crate::providers::ProviderId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelDiscoveryOutcome {
    Listed(Vec<String>),
    ManualOnly { reason: String },
    Unavailable { reason: String },
}

pub async fn discover_setup_models(
    eligible: &[ProviderId],
    providers: &ProvidersConfig,
) -> HashMap<ProviderId, ModelDiscoveryOutcome> {
    discover_setup_models_with(eligible.iter().copied(), |provider| async move {
        match provider {
            ProviderId::OpenRouter => Err("manual-only".to_owned()),
            ProviderId::OpenAI => list_openai_models(&providers.openai).await,
            ProviderId::Anthropic => list_anthropic_models(&providers.anthropic).await,
            ProviderId::Gemini => list_gemini_models(&providers.gemini).await,
            ProviderId::DeepSeek => list_deepseek_models(&providers.deepseek).await,
        }
    })
    .await
}

const DISCOVERY_TIMEOUT_SECS: u64 = 10;

async fn discover_setup_models_with<I, F, Fut>(
    eligible: I,
    load: F,
) -> HashMap<ProviderId, ModelDiscoveryOutcome>
where
    I: IntoIterator<Item = ProviderId>,
    F: Fn(ProviderId) -> Fut + Copy,
    Fut: Future<Output = Result<ModelList, String>>,
{
    use std::time::Duration;
    use tokio::time::timeout;

    collect_discovery_outcomes(eligible.into_iter(), |provider| async move {
        let outcome = match provider {
            ProviderId::OpenRouter => manual_only_outcome(provider),
            _ => match timeout(Duration::from_secs(DISCOVERY_TIMEOUT_SECS), load(provider)).await {
                Ok(Ok(models)) => normalize_model_list(provider, models),
                Ok(Err(err)) => unavailable_from_error(provider, &err),
                Err(_elapsed) => ModelDiscoveryOutcome::Unavailable {
                    reason: format!(
                        "Listing for {} timed out; enter the model manually.",
                        provider.as_str()
                    ),
                },
            },
        };
        (provider, outcome)
    })
    .await
}
```

Implementation notes:

- Use `rig::client::ModelListingClient` and each provider’s `list_models().await` path.
- Build provider clients the same way Scorpio builds completion clients: API key required, optional `base_url` honored.
- For `openrouter`, do not create a client and do not make a network call.
- Convert `ModelList.data.into_iter().map(|model| model.id)` into ordered `Vec<String>`.
- If `list_models()` succeeds but returns no items, convert that to `Unavailable` with the exact CLI-facing message from the spec.
- Keep the test seam (`discover_setup_models_with`) private to the module; it exists only so the public best-effort behavior can be tested without network calls.
- Wrap each provider call inside `discover_setup_models_with` with `tokio::time::timeout(Duration::from_secs(DISCOVERY_TIMEOUT_SECS), load(provider))` so a single slow provider cannot stall the whole batch. Map the timeout error to `Unavailable { reason: "Listing for <provider> timed out; enter the model manually." }`. The `discover_setup_models_with_times_out_slow_providers_without_blocking_others` test pins this with `tokio::time::pause()` and a 30s sleep — assert the slow provider degrades to `Unavailable` while the fast provider still returns `Listed`.
- `unavailable_from_error` MUST construct the user-facing reason from a fixed template that ignores the upstream error string entirely: `format!("Could not load models for {}; enter the model manually.", provider.as_str())`. Do NOT include any substring of the `error: &str` argument in the reason; the argument exists only for diagnostic logging — and that logging MUST also be sanitized. Use the existing `crate::providers::factory::error::sanitize_error_summary(err)` helper before emitting via tracing: `tracing::warn!(provider = provider.as_str(), error = %sanitize_error_summary(err), "list_models failed")`. Do NOT log `error = %err` — the project ships JSON-formatted tracing output (`tracing-subscriber` `json` feature), and operators routinely forward those logs to aggregators. A raw upstream error containing `Bearer sk-…` would leak into durable log storage even though the CLI message stays clean. The fixed template protects the CLI channel; `sanitize_error_summary` extends the same protection to the tracing channel. The `unavailable_reason_uses_fixed_template_regardless_of_upstream_error_shape` negative-leak test locks the CLI contract by asserting the reason is byte-for-byte equal to the fixed template across multiple synthetic leak shapes (Bearer, x-api-key, Authorization, query-string `api_key=`, raw `sk-` substring, JSON-embedded). The 120-character cap is implicitly satisfied by the template and asserted in the same test.
- If `rig-core 0.36.0` does not expose `deepseek.list_models()`, stop immediately because the dependency baseline is wrong.

Update `crates/scorpio-core/src/providers/factory/mod.rs` at the same time:

```rust
//! | [`discovery`] | setup-only provider model listing and normalized discovery outcomes |

mod discovery;

pub use discovery::{ModelDiscoveryOutcome, discover_setup_models};
```

- [ ] **Step 4: Re-run the discovery slice**

Run:

```bash
cargo nextest run -p scorpio-core --all-features --locked -E 'test(openrouter_returns_manual_only) | test(normalize_model_list_preserves_order_and_duplicates) | test(normalize_empty_model_list_returns_unavailable) | test(unavailable_reason_is_sanitized) | test(collect_outcomes_keeps_one_result_per_provider) | test(discover_setup_models_with_sanitizes_failures_and_preserves_successes) | test(discover_setup_models_with_times_out_slow_providers_without_blocking_others) | test(unavailable_reason_uses_fixed_template_regardless_of_upstream_error_shape)'
```

Expected: PASS.

- [ ] **Step 5: Commit the discovery module**

Run:

```bash
git add crates/scorpio-core/src/providers/factory/mod.rs crates/scorpio-core/src/providers/factory/discovery.rs
git commit -m "feat(setup): add provider model discovery in core"
```

## Chunk 3: CLI Step 4 Model Selection, Docs, and Verification

### Task 5: Move step-4 model selection into a focused helper module and use prefetched discovery results

**Files:**
- Modify: `crates/scorpio-cli/src/cli/setup/mod.rs`
- Create: `crates/scorpio-cli/src/cli/setup/model_selection.rs`
- Modify: `crates/scorpio-cli/src/cli/setup/steps.rs`

- [ ] **Step 1: Write the failing step-4 helper tests in the new module**

Create `crates/scorpio-cli/src/cli/setup/model_selection.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_core::providers::ProviderId;
    use scorpio_core::providers::factory::ModelDiscoveryOutcome;

    #[test]
    fn default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported() {
        let eligible = vec![ProviderId::OpenAI, ProviderId::Anthropic, ProviderId::DeepSeek];
        assert_eq!(default_provider_index(&eligible, Some("copilot")), 0);
    }

    #[test]
    fn listed_model_options_put_saved_model_first_and_manual_last() {
        let options = listed_model_options(
            &["gpt-4o-mini".into(), "o3".into(), "gpt-4o-mini".into()],
            Some("o3"),
        );
        assert_eq!(
            options,
            vec![
                ModelMenuOption::Listed("o3".into()),
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Manual,
            ]
        );
    }

    #[test]
    fn listed_model_options_keep_provider_order_when_saved_model_missing() {
        let options = listed_model_options(&["gpt-4o-mini".into(), "o3".into()], Some("claude-opus"));
        assert_eq!(
            options,
            vec![
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Listed("o3".into()),
                ModelMenuOption::Manual,
            ]
        );
    }

    #[test]
    fn prompt_mode_defaults_picker_to_manual_when_saved_model_not_listed() {
        let mode = prompt_mode_for_provider(
            ProviderId::OpenAI,
            &ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]),
            Some("openai"),
            Some("o3"),
        );

        assert_eq!(
            mode,
            ModelPromptMode::Select {
                options: vec![
                    ModelMenuOption::Listed("gpt-4o-mini".into()),
                    ModelMenuOption::Manual,
                ],
                default_index: 1,
            }
        );
    }

    #[test]
    fn manual_prefill_uses_saved_model_only_after_manual_option_is_selected() {
        let initial = manual_initial_value(
            ProviderId::OpenAI,
            Some("openai"),
            Some("o3"),
        );

        assert_eq!(initial, "o3");
    }

    #[test]
    fn prompt_mode_uses_unavailable_note_for_failed_listing() {
        let mode = prompt_mode_for_provider(
            ProviderId::Gemini,
            &ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for gemini; enter the model manually.".into(),
            },
            Some("gemini"),
            Some("gemini-2.5-pro"),
        );

        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some("Could not load models for gemini; enter the model manually.".into()),
                initial_value: "gemini-2.5-pro".into(),
            }
        );
    }

    #[test]
    fn prompt_mode_manual_only_skips_picker_and_goes_straight_to_text_entry() {
        let mode = prompt_mode_for_provider(
            ProviderId::OpenRouter,
            &ModelDiscoveryOutcome::ManualOnly {
                reason: "Model listing is manual-only for openrouter; enter the model manually.".into(),
            },
            Some("openrouter"),
            Some("qwen/qwen3.6-plus-preview:free"),
        );

        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some("Model listing is manual-only for openrouter; enter the model manually.".into()),
                initial_value: "qwen/qwen3.6-plus-preview:free".into(),
            }
        );
    }

    #[test]
    fn prompt_mode_does_not_prefill_saved_model_when_saved_provider_differs() {
        let mode = prompt_mode_for_provider(
            ProviderId::Anthropic,
            &ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for anthropic; enter the model manually.".into(),
            },
            Some("openai"),
            Some("gpt-4o-mini"),
        );

        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some("Could not load models for anthropic; enter the model manually.".into()),
                initial_value: String::new(),
            }
        );
    }

    #[test]
    fn build_provider_routing_plan_reuses_one_prefetched_snapshot_for_both_tiers() {
        let eligible = vec![ProviderId::OpenAI, ProviderId::DeepSeek];
        let discovery = std::collections::HashMap::from([
            (ProviderId::OpenAI, ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()])),
            (ProviderId::DeepSeek, ModelDiscoveryOutcome::Listed(vec!["deepseek-chat".into()])),
        ]);

        let plan = build_provider_routing_plan(
            &eligible,
            &discovery,
            Some("openai"),
            Some("gpt-4o-mini"),
            Some("deepseek"),
            Some("deepseek-chat"),
        );

        assert_eq!(plan.quick.provider, ProviderId::OpenAI);
        assert_eq!(plan.deep.provider, ProviderId::DeepSeek);
        assert!(matches!(plan.quick.prompt_mode, ModelPromptMode::Select { .. }));
        assert!(matches!(plan.deep.prompt_mode, ModelPromptMode::Select { .. }));
    }
}
```

- [ ] **Step 2: Run the CLI slice and verify it fails**

Run:

```bash
cargo nextest run -p scorpio-cli --all-features --locked -E 'test(default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported) | test(listed_model_options_put_saved_model_first_and_manual_last) | test(listed_model_options_keep_provider_order_when_saved_model_missing) | test(prompt_mode_defaults_picker_to_manual_when_saved_model_not_listed) | test(manual_prefill_uses_saved_model_only_after_manual_option_is_selected) | test(prompt_mode_uses_unavailable_note_for_failed_listing) | test(prompt_mode_manual_only_skips_picker_and_goes_straight_to_text_entry) | test(prompt_mode_does_not_prefill_saved_model_when_saved_provider_differs) | test(build_provider_routing_plan_reuses_one_prefetched_snapshot_for_both_tiers)'
```

Expected: FAIL because `model_selection.rs` and its helper types do not exist yet.

- [ ] **Step 3: Implement the step-4 helper module and keep `steps.rs` thin**

Add `mod model_selection;` in `crates/scorpio-cli/src/cli/setup/mod.rs`, then build a focused helper module:

```rust
// crates/scorpio-cli/src/cli/setup/model_selection.rs
use std::collections::HashMap;
use std::path::Path;

use scorpio_core::config::Config;
use scorpio_core::providers::factory::{ModelDiscoveryOutcome, discover_setup_models};
use scorpio_core::providers::ProviderId;
use scorpio_core::settings::PartialConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelMenuOption {
    Listed(String),
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelPromptMode {
    Select { options: Vec<ModelMenuOption>, default_index: usize },
    Manual { note: Option<String>, initial_value: String },
}

struct TierRoutingPlan {
    provider: ProviderId,
    prompt_mode: ModelPromptMode,
}

struct ProviderRoutingPlan {
    quick: TierRoutingPlan,
    deep: TierRoutingPlan,
}

pub fn prompt_provider_routing(
    partial: &mut PartialConfig,
    eligible: Vec<ProviderId>,
    config_path: &Path,
) -> Result<(), inquire::InquireError> {
    let discovery = discover_setup_models_blocking(config_path, partial, &eligible);

    let quick_provider = prompt_provider(
        "Quick-thinking provider (used by analyst agents):",
        &eligible,
        partial.quick_thinking_provider.as_deref(),
    )?;
    let quick_model = prompt_model_for_provider(
        quick_provider,
        discovery.get(&quick_provider).expect("provider outcome exists"),
        partial.quick_thinking_provider.as_deref(),
        partial.quick_thinking_model.as_deref(),
    )?;

    let deep_provider = prompt_provider(
        "Deep-thinking provider (used by researcher, trader, and risk agents):",
        &eligible,
        partial.deep_thinking_provider.as_deref(),
    )?;
    let deep_model = prompt_model_for_provider(
        deep_provider,
        discovery.get(&deep_provider).expect("provider outcome exists"),
        partial.deep_thinking_provider.as_deref(),
        partial.deep_thinking_model.as_deref(),
    )?;

    super::steps::apply_provider_routing(partial, (quick_provider, quick_model), (deep_provider, deep_model));
    Ok(())
}
```

Implementation notes:

- `steps.rs` should delegate `step4_provider_routing` to `model_selection::prompt_provider_routing`.
- Pass the existing user config path from `setup/mod.rs` into `step4_provider_routing`, then into `model_selection::prompt_provider_routing`, so discovery can call `Config::load_effective_providers_config_from_user_path(config_path, partial)` and preserve file-backed `[providers.*]` overrides while ignoring stale Copilot routing.
- Build a current-thread Tokio runtime inside the blocking bootstrap helper and call `discover_setup_models` once.
- If provider-settings loading or async discovery fails before any per-provider results exist, synthesize `Unavailable` outcomes for every eligible provider using the spec’s CLI message contract.
- Keep `WIZARD_PROVIDERS` order as the source of truth for the “first eligible provider” fallback.
- `step4_provider_routing` may assume `eligible` is non-empty because step 3 already enforces that at least one keyed provider exists before step 4 runs.
- Keep `provider_key`/`set_provider_key` free of Copilot branches after the provider enum cleanup.
- Manual model entry validation stays exactly as today: non-empty input only.
- Add focused pure helpers instead of embedding all branching in one function:
  - `default_provider_index(...)`
  - `listed_model_options(...)`
  - `prompt_mode_for_provider(...)`
  - `manual_initial_value(...)`
  - `prompt_provider(...)`
  - `prompt_model_for_provider(...)`
  - `discover_setup_models_blocking(...)`
  - `build_provider_routing_plan(...)` for testable quick/deep plan assembly from one prefetched discovery snapshot

- `prompt_mode_for_provider(...)` must follow the spec precisely:
  - `Listed` with saved model present -> picker with saved model moved to the top
  - `Listed` with saved model absent -> picker still shown, but default cursor moves to `Enter model manually`
  - selecting `Enter model manually` then opens the text prompt, whose initial value comes from `manual_initial_value(...)`
  - `ManualOnly` / `Unavailable` -> skip picker and go straight to the manual text prompt

- [ ] **Step 4: Update `steps.rs` to use the new helper and remove leftover Copilot-only logic**

Refactor `steps.rs` so it becomes:

```rust
pub fn step4_provider_routing(
    config_path: &std::path::Path,
    partial: &mut PartialConfig,
) -> Result<(), inquire::InquireError> {
    let eligible = providers_with_keys(partial);
    super::model_selection::prompt_provider_routing(partial, eligible, config_path)
}

fn provider_key(partial: &PartialConfig, provider: ProviderId) -> Option<&str> {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key.as_deref(),
        ProviderId::Anthropic => partial.anthropic_api_key.as_deref(),
        ProviderId::Gemini => partial.gemini_api_key.as_deref(),
        ProviderId::OpenRouter => partial.openrouter_api_key.as_deref(),
        ProviderId::DeepSeek => partial.deepseek_api_key.as_deref(),
    }
}
```

Update `crates/scorpio-cli/src/cli/setup/mod.rs` at the same time so the orchestrator passes the already-resolved `config_path` into step 4:

```rust
step!(step4_provider_routing(&config_path, &mut partial));
```

Also remove the old Copilot preflight helper call from the health-check path if any references remain.

- [ ] **Step 5: Run the focused CLI slices**

Run:

```bash
cargo nextest run -p scorpio-cli --all-features --locked -E 'test(default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported) | test(listed_model_options_put_saved_model_first_and_manual_last) | test(listed_model_options_keep_provider_order_when_saved_model_missing) | test(prompt_mode_defaults_picker_to_manual_when_saved_model_not_listed) | test(manual_prefill_uses_saved_model_only_after_manual_option_is_selected) | test(prompt_mode_uses_unavailable_note_for_failed_listing) | test(prompt_mode_manual_only_skips_picker_and_goes_straight_to_text_entry) | test(prompt_mode_does_not_prefill_saved_model_when_saved_provider_differs) | test(build_provider_routing_plan_reuses_one_prefetched_snapshot_for_both_tiers) | test(providers_with_keys_all_set_returns_all_wizard_providers) | test(provider_id_display_matches_as_str)'
```

Expected: PASS.

- [ ] **Step 6: Commit the step-4 UX change**

Run:

```bash
git add crates/scorpio-cli/src/cli/setup/mod.rs crates/scorpio-cli/src/cli/setup/model_selection.rs crates/scorpio-cli/src/cli/setup/steps.rs
git commit -m "feat(setup): list provider models in step 4"
```

### Task 6: Update README and run the repo-standard verification sequence

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the setup and limitation text in `README.md`**

Replace the outdated setup note and limitation section with factual copy matching the spec. The resulting README should say all of the following:

- setup step 4 can list provider models for supported keyed providers
- `openrouter` remains manual-only
- manual model entry is always available
- custom Copilot support is temporarily unavailable in this slice and will return later through official `rig` support

Use copy in this shape:

```md
> **Note:** `scorpio setup` now fetches model lists for supported keyed providers during step 4. OpenRouter remains manual-only, and `Enter model manually` is always available.

**GitHub Copilot is temporarily unavailable in this build**

The previous custom ACP-based Copilot provider was removed as part of the `rig-core 0.36.0` upgrade. A follow-up change will reintroduce Copilot through upstream `rig` support.
```

- [ ] **Step 2: Verify the README text changed in the intended places**

Run:

```bash
rg -n "Enter model manually|manual-only|temporarily unavailable|rig support" README.md
if rg -n "Copilot provider does not yet support tool calling|ACP \(Agent Client Protocol\)|session/new" README.md; then exit 1; fi
```

Expected:

- The new setup-step wording is present.
- The old “Copilot provider does not yet support tool calling” ACP wording is gone because the second command prints no matches and exits successfully.

- [ ] **Step 3: Run the full verification sequence**

Run these commands in order:

```bash
cargo fmt -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace --all-features --locked --no-fail-fast
```

Expected:

- All three commands PASS.
- If any failure appears unrelated to this slice, stop and use `@superpowers:systematic-debugging` before editing more code.

- [ ] **Step 4: Commit the docs and final green verification state**

Run:

```bash
git add README.md
git commit -m "docs(setup): document model listing and temporary copilot removal"
```
