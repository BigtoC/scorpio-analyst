# Config-driven snapshot_db_path Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the SQLite snapshot database path configurable via `config.toml` and env vars, with a default of `~/.scorpio-analyst/phase_snapshots.db`, and fix a latent bug in the env var hierarchy separator.

**Architecture:** Add a `StorageConfig` struct to `src/config.rs` with a `snapshot_db_path: String` field; add a `pub fn expand_path(s: &str) -> PathBuf` free function that resolves `~/` and `$HOME/` at runtime; fix `.separator("_")` → `.separator("__")` in `Config::load_from` so nested env var overrides work correctly. `SnapshotStore` is untouched.

**Tech Stack:** Rust 1.93+ (edition 2024), `config` 0.15 crate, `serde`/`serde_json`, `tokio`, `tracing`.

**Spec:** `docs/superpowers/specs/2026-03-19-config-driven-snapshot-db-path-design.md`

---

## Files modified

| File            | What changes                                                                                                                   |
|-----------------|--------------------------------------------------------------------------------------------------------------------------------|
| `src/config.rs` | Fix `.separator("__")`, add `StorageConfig` + `Default`, add `storage` field on `Config`, add `expand_path` fn, add unit tests |
| `config.toml`   | Add `[storage]` section; update prefix comment                                                                                 |

No other files change. `src/workflow/snapshot.rs`, `src/workflow/tasks.rs`, `tests/workflow_pipeline.rs`, `tests/workflow_observability.rs` — all unchanged.

---

## Chunk 1: Fix separator + add StorageConfig

### Task 1: Fix the env var hierarchy separator (latent bug fix)

**Files:**
- Modify: `src/config.rs:146` (the `.separator("_")` call)

The `config` crate's `Environment::separator` determines how nested keys are encoded in env var names. With `"_"`, the env var `SCORPIO_LLM_MAX_DEBATE_ROUNDS` is parsed as path `llm → max → debate → rounds`, which doesn't match `LlmConfig.max_debate_rounds`. Changing to `"__"` means only double-underscores split hierarchy levels, so single underscores in field names are preserved.

- [ ] **Step 1: Write the failing test**

Add this test to the `#[cfg(test)] mod tests` block at the bottom of `src/config.rs`:

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

Also add the shared lock near the top of the `tests` module (above all test functions):

```rust
/// Serializes tests that mutate environment variables.
/// `std::env::set_var` is not thread-safe; all tests touching env vars must
/// hold this lock for the duration of the test.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test env_override_uses_double_underscore_separator -- --nocapture
```

Expected: FAIL — the override is not picked up because `.separator("_")` doesn't parse `SCORPIO__LLM__MAX_DEBATE_ROUNDS` correctly.

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

Expected: all tests pass. (No existing tests rely on env var overrides going through the `config` crate — the secret API keys are loaded via direct `std::env::var` calls and are unaffected.)

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "fix(config): change env separator from _ to __ to support nested keys with underscores"
```

---

### Task 2: Add `StorageConfig` struct and wire it into `Config`

**Files:**
- Modify: `src/config.rs` (add struct, default fn, `Default` impl, field on `Config`)

- [ ] **Step 1: Write the failing test**

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

Then add the `storage` field to the `Config` struct:

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

## Chunk 2: expand_path + config.toml

### Task 3: Add `expand_path` function with unit tests

**Files:**
- Modify: `src/config.rs` (add `pub fn expand_path`, add tests)

`expand_path` is a pure function. It does not call any I/O other than `std::env::var("HOME")`. It does not write to the filesystem or create directories. Directory creation is the caller's responsibility.

Rules:
1. If `s` starts with `~/`, replace `~` with `$HOME` (env var). If `HOME` is unset, emit `tracing::warn!` and use `.` (current dir) as fallback.
2. If `s` starts with `$HOME/`, substitute `$HOME`. Same fallback.
3. Otherwise return `PathBuf::from(s)` unchanged.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/config.rs`:

```rust
#[test]
fn expand_path_tilde_prefix() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", "/home/testuser") };
    let result = expand_path("~/foo/bar.db");
    unsafe { std::env::remove_var("HOME") };
    assert_eq!(result, std::path::PathBuf::from("/home/testuser/foo/bar.db"));
}

#[test]
fn expand_path_dollar_home_prefix() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("HOME", "/home/testuser") };
    let result = expand_path("$HOME/foo/bar.db");
    unsafe { std::env::remove_var("HOME") };
    assert_eq!(result, std::path::PathBuf::from("/home/testuser/foo/bar.db"));
}

#[test]
fn expand_path_absolute_unchanged() {
    let result = expand_path("/absolute/path.db");
    assert_eq!(result, std::path::PathBuf::from("/absolute/path.db"));
}

#[test]
fn expand_path_relative_unchanged() {
    let result = expand_path("relative/path.db");
    assert_eq!(result, std::path::PathBuf::from("relative/path.db"));
}

#[test]
fn expand_path_home_unset_falls_back_to_dot() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("HOME") };
    let result = expand_path("~/foo/bar.db");
    // When HOME is unset, falls back to "." + the suffix
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
/// - If `HOME` is unset, falls back to `"."` with a warning.
/// - All other paths are returned as-is.
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

Expected: all 5 tests PASS.

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
- Modify: `config.toml` (add `[storage]` section, fix prefix comment)

- [ ] **Step 1: Add the `[storage]` section to `config.toml`**

Open `config.toml`. The current content ends with:

```toml
[api]
finnhub_rate_limit = 30
```

Append the following, and also update the comment on line 2 from `SCORPIO_` to `SCORPIO__`:

```toml
# Scorpio Analyst — default configuration
# Override with .env or environment variables (prefix: SCORPIO__, e.g. SCORPIO__LLM__MAX_DEBATE_ROUNDS=5)

[llm]
...

[storage]
# Path to the SQLite snapshot database. Supports ~ and $HOME expansion.
# Override: SCORPIO__STORAGE__SNAPSHOT_DB_PATH=/your/custom/path.db
snapshot_db_path = "~/.scorpio-analyst/phase_snapshots.db"
```

The final `config.toml` should look like:

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

Expected: all tests pass. The existing `load_from_defaults_only` test checks `max_debate_rounds` and `finnhub_rate_limit` — both unaffected. The `storage_config_defaults_to_tilde_path` test added in Task 2 will also pass since `config.toml` now has an explicit `[storage]` value matching the default.

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

## Final verification

- [ ] **Run the complete test suite one last time**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check
```

Expected: all tests pass, no clippy warnings, formatting clean.
