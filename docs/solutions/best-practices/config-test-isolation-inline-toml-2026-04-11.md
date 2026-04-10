---
title: "Config Tests Must Use Inline TOML, Not the Production config.toml"
date: 2026-04-11
category: docs/solutions/best-practices
module: config
problem_type: best_practice
component: testing_framework
severity: medium
applies_when:
  - A test in src/config.rs calls Config::load_from() against the production config.toml
  - A config default value in config.toml is legitimately changed (e.g. rate-limit tuning)
  - A new test needs to assert a specific parsed config value or env-var override
tags:
  - test-isolation
  - config
  - inline-toml
  - tempfile
  - rust
  - best-practice
  - config-loading
  - fragile-tests
---

# Config Tests Must Use Inline TOML, Not the Production config.toml

## Context

`src/config.rs` contains a `Config::load_from(path)` helper that loads configuration from a
TOML file. Several unit tests were calling `Config::load_from("config.toml")` — the live
production file — instead of an isolated fixture. This created silent coupling between test
assertions and production defaults.

When `yahoo_finance_rps` was bumped from `10` to `30` in `config.toml` (a valid, intentional
tuning change), two tests failed:

- `rate_limit_config_default_has_yahoo_finance_rps_10` — expected `10`, got `30`
- `load_from_defaults_only` — same stale assertion

A reader of the test could not determine the expected value without also consulting the
production file. Tests were not hermetic: they depended on a mutable external file rather than
owning their input.

## Guidance

**Never reference `config.toml` from unit tests.** Instead:

### 1. Define a `MINIMAL_CONFIG_TOML` constant

Include only the fields that have no `#[serde(default)]` and must satisfy `validate()`.
Everything else falls through to compiled-in Rust defaults.

```rust
/// Minimum valid TOML: only the fields that have no `serde(default)` and
/// are required by `validate()`. All other fields fall through to their
/// compiled-in defaults, keeping tests independent of `config.toml`.
const MINIMAL_CONFIG_TOML: &str = r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"

[trading]
asset_symbol = "AAPL"
"#;
```

### 2. Add a `write_config` helper

Writes TOML into a `tempfile::TempDir` and returns both the dir and the path. The `TempDir`
must be bound to a named variable so it is not dropped before the test completes.

```rust
fn write_config(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, content).expect("config file should be written");
    (dir, path)
}
```

### 3. For tests asserting specific values, use a self-contained inline TOML

Declare every value being asserted explicitly in the TOML. The test becomes its own
documentation and cannot drift independently from `config.toml`.

## Why This Matters

| Risk | Production-file coupling | Inline TOML |
|---|---|---|
| Test breaks after unrelated file change | Yes | No |
| Reader must consult external file to understand assertions | Yes | No |
| Hermetic in CI (no dependency on CWD or file presence) | No | Yes |
| Self-documenting expected values | No | Yes |

The specific failure pattern is: a legitimate tuning change to `config.toml` (e.g. bumping a
rate-limit default) silently cascades into test failures in an entirely unrelated test run,
with no obvious connection between the change and the failure.

## When to Apply

- Any test that calls `Config::load_from()`, `Config::load()`, or any config-parsing function
  that reads a file also used in production.
- Any test that asserts a specific numeric or string default that could legitimately change
  without being a bug.
- Any test that sets environment variables and then loads config — these must be doubly
  isolated: from the production file and from concurrent env-var mutations via `ENV_LOCK`.
- When adding a new required config field (no `serde(default)`): update `MINIMAL_CONFIG_TOML`
  to include it. For fields with `serde(default)`, leave the minimal config unchanged.

## Examples

### Before — coupled to production file

```rust
#[test]
fn rate_limit_config_default_has_yahoo_finance_rps_10() {
    // Breaks the moment config.toml bumps yahoo_finance_rps to 30
    let cfg = Config::load_from("config.toml").expect("should load");
    assert_eq!(cfg.rate_limits.yahoo_finance_rps, 10);
}

#[test]
fn env_override_uses_double_underscore_separator() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS", "7"); }
    // Still reads the production file as base
    let result = Config::load_from("config.toml");
    unsafe { std::env::remove_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS"); }
    assert_eq!(result.unwrap().llm.max_debate_rounds, 7);
}
```

### After — isolated, self-contained

```rust
#[test]
fn rate_limit_config_default_has_yahoo_finance_rps_30() {
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML);
    let cfg = Config::load_from(&path).expect("should load");
    // Value resolves from compiled-in serde default, not config.toml
    assert_eq!(cfg.rate_limits.yahoo_finance_rps, 30);
}

#[test]
fn env_override_uses_double_underscore_separator() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (_dir, path) = write_config(MINIMAL_CONFIG_TOML); // isolated base
    unsafe { std::env::set_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS", "7"); }
    let result = Config::load_from(&path);
    unsafe { std::env::remove_var("SCORPIO__LLM__MAX_DEBATE_ROUNDS"); }
    assert_eq!(result.unwrap().llm.max_debate_rounds, 7);
}

#[test]
fn load_from_defaults_only() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Every value asserted below is declared in this TOML.
    // The test is its own documentation; it cannot drift with config.toml.
    let (_dir, path) = write_config(r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
analyst_timeout_secs = 3000

[trading]
asset_symbol = "AAPL"

[rate_limits]
finnhub_rps = 30
fred_rps = 2
yahoo_finance_rps = 30

[providers.openai]
rpm = 500

[providers.anthropic]
rpm = 500

[providers.gemini]
rpm = 500

[providers.copilot]
rpm = 0

[providers.openrouter]
rpm = 20
"#);
    let cfg = Config::load_from(&path).expect("config should load");
    assert_eq!(cfg.llm.max_debate_rounds, 3);       // compiled-in default
    assert_eq!(cfg.llm.analyst_timeout_secs, 3000); // declared in TOML above
    assert_eq!(cfg.rate_limits.yahoo_finance_rps, 30);
    assert_eq!(cfg.providers.openai.rpm, 500);
}
```

### Key mechanical notes

- `write_config` returns `(TempDir, PathBuf)`. Always bind `TempDir` to a named `_dir` variable.
  Unnamed temporaries are dropped at the end of the `let` statement, deleting the file before
  it can be read.
- `MINIMAL_CONFIG_TOML` omits all `#[serde(default)]` fields. Those fields resolve to their
  `Default` impl — the same value `config.toml` ships with initially, but decoupled from the
  file at test runtime.
- TOML does not allow a table header (`[llm]`) to appear more than once in a document.
  Construct a single self-contained TOML string per test rather than trying to merge the
  minimal base with extra sections.

## Related

- `tempfile` crate (already a dev dependency) — no new deps required
- `src/config.rs` test module — `MINIMAL_CONFIG_TOML` and `write_config` are defined here
- Pattern already used for field-specific tests in `src/config.rs` (lines 601–657 pre-refactor)
