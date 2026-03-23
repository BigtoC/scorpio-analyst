# Config-driven snapshot_db_path Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the SQLite snapshot database path configurable via `config.toml` and env vars, with a default of `~/.scorpio-analyst/phase_snapshots.db`, and fix a latent bug in the env var hierarchy separator.

**Architecture:** Add a `StorageConfig` struct to `src/config.rs` with a `snapshot_db_path: String` field; add a `pub fn expand_path(s: &str) -> PathBuf` free function that resolves `~/` and `$HOME/` at runtime; fix `.separator("_")` → `.separator("__")` in `Config::load_from` so nested env var overrides work correctly; log the resolved path at startup in `main.rs`. `SnapshotStore` is untouched.

**Tech Stack:** Rust 1.93+ (edition 2024), `config` 0.15 crate, `serde`/`serde_json`, `tokio`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-03-19-config-driven-snapshot-db-path-design.md`

---

## Files modified

| File            | What changes                                                                                                                   |
|-----------------|--------------------------------------------------------------------------------------------------------------------------------|
| `src/config.rs` | Fix `.separator("__")`, add `StorageConfig` + `Default`, add `storage` field on `Config`, add `expand_path` fn, add unit tests |
| `config.toml`   | Add `[storage]` section; update prefix comment                                                                                 |
| `src/main.rs`   | Call `expand_path` at startup and log the resolved path                                                                        |

No other files change. `src/workflow/snapshot.rs`, `src/workflow/tasks.rs`, `tests/workflow_pipeline.rs`, `tests/workflow_observability.rs` — all unchanged.

---

## Chunk 1: Fix separator + add StorageConfig

### Task 1: Fix the env var hierarchy separator (latent bug fix)

**Files:**
- Modify: `src/config.rs:146` (the `.separator("_")` call)

The `config` crate's `Environment::separator` determines how nested keys are encoded in env var names. With `"_"`, the env var `SCORPIO_LLM_MAX_DEBATE_ROUNDS` is parsed as path `llm → max → debate → rounds`, which doesn't match `LlmConfig.max_debate_rounds`. Changing to `"__"` means only double-underscores split hierarchy levels, so single underscores in field names are preserved.

- [ ] **Step 1: Write the failing test**

Add this test to the `#[cfg(test)] mod tests` block at the bottom of `src/config.rs`.

First, add a shared mutex for env-var tests near the top of the `tests` module (above all test functions). This serializes all tests that mutate env vars, preventing races since `std::env::set_var` is not thread-safe:

```rust
/// Serializes tests that mutate environment variables.
/// `std::env::set_var` is not thread-safe; all tests touching env vars must
/// hold this lock for the duration of the test.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

Then add the test:

```rust
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
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test env_override_uses_double_underscore_separator -- --nocapture
```

Expected: FAIL — the override is not picked up because `.separator("_")` doesn't parse `SCORPIO__LLM__MAX_DEBATE_ROUNDS` correctly (the `config` crate sees empty segments around `__` when splitting on `_`).

- [ ] **Step 3: Fix the separator**

In `src/config.rs`, find the `Config::load_from` method (around line 143). Change:

```rust
config::Environment::with_prefix("SCORPIO")
    .separator("_")
    .try_parsing(true),
```

to:

```rust
config::Environment::with_prefix("SCORPIO")
    .separator("__")
    .try_parsing(true),
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test env_override_uses_double_underscore_separator -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Run the full test suite to make sure nothing regressed**

```bash
cargo test
```

Expected: all tests pass. (No existing tests rely on env var overrides going through the `config` crate — the secret API keys are loaded via direct `std::env::var` calls and are unaffected by the separator change.)

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "fix(config): change env separator from _ to __ to support nested keys with underscores"
```

---

### Task 2: Add `StorageConfig` struct and wire it into `Config`

**Files:**
- Modify: `src/config.rs` (add struct, default fn, `Default` impl, field on `Config`)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/config.rs`:

```rust
#[test]
fn storage_config_defaults_to_tilde_path() {
    let cfg = Config::load_from("config.toml").expect("config should load");
    assert_eq!(
        cfg.storage.snapshot_db_path,
        "~/.scorpio-analyst/phase_snapshots.db",
        "default snapshot_db_path should be the tilde-prefixed path"
    );
}

#[test]
fn storage_config_can_be_overridden_via_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var(
            "SCORPIO__STORAGE__SNAPSHOT_DB_PATH",
            "/tmp/custom.db",
        );
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
```

- [ ] **Step 2: Run the tests to confirm they fail**

```bash
cargo test storage_config -- --nocapture
```

Expected: compile error — `Config` has no `storage` field yet.

- [ ] **Step 3: Add `StorageConfig` and wire it in**

In `src/config.rs`, add the following after the `ApiConfig` block (before `fn default_finnhub_rate_limit`):

```rust
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
```

Then add `storage` as the last field on the `Config` struct (after `api`):

```rust
pub struct Config {
    pub llm: LlmConfig,
    pub trading: TradingConfig,
    pub api: ApiConfig,
    #[serde(default)]
    pub storage: StorageConfig,
}
```

- [ ] **Step 4: Run the tests to confirm they pass**

```bash
cargo test storage_config -- --nocapture
```

Expected: both tests PASS.

- [ ] **Step 5: Run the full suite and clippy**

```bash
cargo test && cargo clippy -- -D warnings
```

Expected: all pass, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add StorageConfig with snapshot_db_path field"
```

---

## Chunk 2: expand_path + config.toml + main.rs wiring

### Task 3: Add `expand_path` function with unit tests

**Files:**
- Modify: `src/config.rs` (add `pub fn expand_path`, add tests)

`expand_path` is a pure function. It does not call any I/O other than `std::env::var("HOME")`. It does not write to the filesystem or create directories. Directory creation is the caller's responsibility.

Rules:
1. If `s` starts with `~/`, replace `~` with `$HOME` env var. If `HOME` is unset, emit `tracing::warn!` and use `"."` as fallback.
2. If `s` starts with `$HOME/`, substitute `$HOME` env var. Same fallback.
3. Otherwise, return `PathBuf::from(s)` unchanged.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/config.rs`. Note that `expand_path_absolute_unchanged` and `expand_path_relative_unchanged` do not acquire `ENV_LOCK` — inputs that don't start with `~/` or `$HOME/` never read `HOME`, so they are safe to run concurrently:

```rust
#[test]
fn expand_path_tilde_prefix() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: serialized by ENV_LOCK
    unsafe { std::env::set_var("HOME", "/home/testuser") };
    let result = expand_path("~/foo/bar.db");
    unsafe { std::env::remove_var("HOME") };
    assert_eq!(result, std::path::PathBuf::from("/home/testuser/foo/bar.db"));
}

#[test]
fn expand_path_dollar_home_prefix() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: serialized by ENV_LOCK
    unsafe { std::env::set_var("HOME", "/home/testuser") };
    let result = expand_path("$HOME/foo/bar.db");
    unsafe { std::env::remove_var("HOME") };
    assert_eq!(result, std::path::PathBuf::from("/home/testuser/foo/bar.db"));
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
    unsafe { std::env::remove_var("HOME") };
    let result = expand_path("~/foo/bar.db");
    // Fallback home is "." so format!("{home}/{rest}") == "./foo/bar.db"
    assert_eq!(result, std::path::PathBuf::from("./foo/bar.db"));
}

#[test]
fn expand_path_dollar_home_unset_falls_back_to_dot() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: serialized by ENV_LOCK
    unsafe { std::env::remove_var("HOME") };
    let result = expand_path("$HOME/foo/bar.db");
    // Fallback home is "." so format!("{home}/{rest}") == "./foo/bar.db"
    assert_eq!(result, std::path::PathBuf::from("./foo/bar.db"));
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

```bash
cargo test expand_path -- --nocapture
```

Expected: compile error — `expand_path` is not defined yet.

- [ ] **Step 3: Implement `expand_path`**

Add this function to `src/config.rs`, after the `StorageConfig` block (before `impl Config`):

```rust
/// Resolve `~/` and `$HOME/` prefix in a path string to the actual home directory.
///
/// - `~/foo` and `$HOME/foo` both expand using the `HOME` environment variable.
/// - If `HOME` is unset, falls back to `"."` with a warning logged via `tracing::warn!`.
/// - All other paths are returned as-is (absolute and relative paths pass through unchanged).
pub fn expand_path(s: &str) -> std::path::PathBuf {
    let suffix = if let Some(rest) = s.strip_prefix("~/") {
        Some(rest)
    } else if let Some(rest) = s.strip_prefix("$HOME/") {
        Some(rest)
    } else {
        None
    };

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
```

- [ ] **Step 4: Run the tests to confirm they all pass**

```bash
cargo test expand_path -- --nocapture
```

Expected: all 6 tests PASS.

- [ ] **Step 5: Run the full suite and clippy**

```bash
cargo test && cargo clippy -- -D warnings
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add expand_path helper for ~ and \$HOME path expansion"
```

---

### Task 4: Update `config.toml`

**Files:**
- Modify: `config.toml`

- [ ] **Step 1: Replace the contents of `config.toml` with the following**

The only changes are: (a) update the comment on line 2 to reflect the new `SCORPIO__` prefix convention; (b) append the `[storage]` section at the end.

```toml
# Scorpio Analyst — default configuration
# Override with .env or environment variables (prefix: SCORPIO__, e.g. SCORPIO__LLM__MAX_DEBATE_ROUNDS=5)

[llm]
quick_thinking_provider = "gemini"
quick_thinking_model = "gemini-2.5-fast"
deep_thinking_provider = "openai"
deep_thinking_model = "gpt-5.4"
max_debate_rounds = 3
max_risk_rounds = 2
agent_timeout_secs = 30

[trading]
asset_symbol = "AAPL"
backtest_start = "2024-06-01"
backtest_end = "2024-11-30"

[api]
finnhub_rate_limit = 30

[storage]
# Path to the SQLite snapshot database. Supports ~ and $HOME expansion.
# Override: SCORPIO__STORAGE__SNAPSHOT_DB_PATH=/your/custom/path.db
snapshot_db_path = "~/.scorpio-analyst/phase_snapshots.db"
```

- [ ] **Step 2: Run the full test suite**

```bash
cargo test
```

Expected: all tests pass. The existing `load_from_defaults_only` test checks `max_debate_rounds` and `finnhub_rate_limit` — both unaffected. The `storage_config_defaults_to_tilde_path` test added in Task 2 also passes since `config.toml` now has an explicit `[storage]` value matching the default.

- [ ] **Step 3: Run clippy and fmt**

```bash
cargo clippy -- -D warnings && cargo fmt -- --check
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add config.toml
git commit -m "feat(config): add [storage] section with snapshot_db_path"
```

---

### Task 5: Wire `expand_path` into `main.rs`

**Files:**
- Modify: `src/main.rs`

`main.rs` does not yet construct a `SnapshotStore` (the full pipeline is not wired up yet), but `expand_path` must be called somewhere in production code to satisfy the spec goal. Add a startup log line that resolves and logs the configured db path. This makes the resolved path visible to operators and keeps `expand_path` and `StorageConfig` from being dead code.

- [ ] **Step 1: Update `main.rs`**

Replace the contents of `src/main.rs` with the following (adds `expand_path` import and a startup log for the resolved snapshot db path):

```rust
use scorpio_analyst::config::{Config, expand_path};
use scorpio_analyst::observability::init_tracing;
use scorpio_analyst::providers::factory::preflight_configured_providers;

fn main() {
    init_tracing();

    match Config::load() {
        Ok(cfg) => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    eprintln!("failed to initialize async runtime: {e:#}");
                    std::process::exit(1);
                }
            };

            if let Err(e) = runtime.block_on(preflight_configured_providers(&cfg.llm, &cfg.api)) {
                eprintln!("failed to preflight configured providers: {e:#}");
                std::process::exit(1);
            }

            let snapshot_db_path = expand_path(&cfg.storage.snapshot_db_path);
            tracing::info!(
                snapshot_db_path = %snapshot_db_path.display(),
                "storage configured"
            );

            tracing::info!(
                quick_provider = %cfg.llm.quick_thinking_provider,
                deep_provider = %cfg.llm.deep_thinking_provider,
                symbol = %cfg.trading.asset_symbol,
                "scorpio-analyst initialized"
            );
        }
        Err(e) => {
            eprintln!("failed to load configuration: {e:#}");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 2: Build to verify it compiles**

```bash
cargo build
```

Expected: compiles cleanly with no warnings.

- [ ] **Step 3: Run the full suite, clippy, and fmt**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): log resolved snapshot_db_path at startup"
```

---

## Final verification

- [ ] **Run the complete test suite one last time**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check && cargo build
```

Expected: all tests pass, no clippy warnings, formatting clean, build succeeds.
