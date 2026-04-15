---
title: "CLI Runtime Config Parity and Setup Health-Check Recovery"
date: 2026-04-15
category: docs/solutions/logic-errors
module: cli
problem_type: logic_error
component: tooling
symptoms:
  - `scorpio analyze <SYMBOL>` rejected valid env-only setups as "Config not found or incomplete"
  - Setup Step 5 could pass or fail on a different effective runtime config than `analyze`
  - Setup surfaced parse and health-check failures in ways that could leak too much detail or leave users blocked on malformed config
root_cause: config_error
resolution_type: code_fix
severity: high
tags:
  - cli
  - config-loading
  - setup-wizard
  - analyze
  - runtime-parity
  - copilot
  - health-check
  - malformed-config
---

# CLI Runtime Config Parity and Setup Health-Check Recovery

## Problem

The new CLI work introduced a cluster of correctness drifts between `scorpio analyze` and `scorpio setup`. The main failures were that env-only analysis stopped working, setup Step 5 was not validating the same effective runtime config that `analyze` actually uses, and malformed user config handling was brittle.

## Symptoms

- `scorpio analyze AAPL` could fail with `Config not found or incomplete` even when the required providers, models, and API keys were present only in environment variables.
- Setup Step 5 could report a green or red result that did not match what a later `analyze` run would do, especially around Copilot preflight and model-route validation.
- A parse-broken `~/.scorpio-analyst/config.toml` could block setup entirely instead of being recoverable by backing it up and starting fresh.
- Setup failure output included more raw provider error text than was appropriate for an interactive health check.

## What Didn't Work

- Treating `Config::load()` for `analyze` and the setup wizard's in-memory `PartialConfig` as separate loading paths. That let the two entrypoints drift on env merging, readiness checks, and provider preflight.
- Hand-shaping nested TOML for setup/runtime bridging in a fragile way. This made the conversion logic easy to break and harder to extend safely.
- Letting malformed config parsing behave like an unrecoverable fatal path instead of a recoverable user-state problem.
- Verifying only the deep-thinking route in Step 5. That missed failures in the quick-thinking route even though `analyze` needs both tiers to be usable.

## Solution

The fix was to collapse both CLI flows onto the same effective runtime assembly and readiness checks, then harden setup's file handling and health-check behavior.

### 1. Introduce one shared effective-runtime loader

`src/config.rs` now exposes `Config::load_effective_runtime(partial)` and uses it as the common merge path for:

- user config values from `PartialConfig`
- `.env` loading via `dotenvy`
- nested `SCORPIO__...` overrides
- flat secret env vars such as `SCORPIO_OPENAI_API_KEY`
- compiled defaults

`Config::load()` now falls back to `load_effective_runtime(PartialConfig::default())` when no user config path is available, which restored env-only `scorpio analyze` support.

### 2. Add one shared readiness gate for analysis

`Config::is_analysis_ready()` now checks the same runtime prerequisites that matter for a real analysis run:

- quick-thinking model route can be created
- deep-thinking model route can be created
- Finnhub client can be created
- FRED client can be created

`src/cli/analyze.rs` uses this gate before running, and `src/cli/setup/steps.rs` calls it inside Step 5 before probing models.

### 3. Make Step 5 preflight match `analyze`

Setup Step 5 now builds the effective runtime config with `Config::load_effective_runtime(partial.clone())`, then:

- runs `cfg.is_analysis_ready()`
- preflights Copilot when configured
- live health-checks both `ModelTier::QuickThinking` and `ModelTier::DeepThinking`

This removed the previous parity gap where setup only proved a subset of the runtime that `analyze` depends on.

### 4. Harden user-config path and serialization behavior

`src/cli/setup/config_file.rs` now fail-closes secret config path resolution when `HOME` is unset or relative, instead of resolving an unsafe or ambiguous path.

`src/config.rs` also replaced panic-prone/manual TOML shaping with structured safe serialization via `toml::Value` plus `toml::to_string(...)` in `partial_to_nested_toml_non_secrets(...)`.

### 5. Recover cleanly from malformed config files

`src/cli/setup/mod.rs` now treats parse-broken user config as a recoverable case:

- prompt the user to move the malformed file aside
- rename it to a timestamped backup
- continue setup from `PartialConfig::default()`

The recovery path keeps the broken file for inspection instead of deleting it, and the prompt/error path is written so secret contents are not echoed.

### 6. Sanitize interactive health-check failures

Setup Step 5 now reports sanitized provider failure summaries instead of dumping raw nested error text into the wizard flow.

## Why This Works

The underlying issue was not one isolated bug. It was a parity failure between two entrypoints that were both trying to answer the question "is this runtime config usable?" with different code paths.

By moving both `analyze` and setup Step 5 onto `Config::load_effective_runtime(...)` plus `Config::is_analysis_ready()`, the repo now has one authoritative definition of the effective runtime and one authoritative readiness contract. The additional Copilot preflight and dual-tier live probing close the remaining gap between "config parses" and "this exact configured runtime can actually analyze".

The malformed-config recovery and sanitized output changes solve the adjacent DX and safety issues without weakening correctness: setup remains strict, but users can recover from a broken file safely and without secret leakage.

## Prevention

- For future CLI work, treat `analyze` and `setup` as parity surfaces. If one adds or tightens a runtime prerequisite, the other must either call the same helper or intentionally document why it differs.
- Add new runtime checks to shared helpers first, not directly inside one command path. In this area, prefer extending `Config::load_effective_runtime(...)` or `Config::is_analysis_ready()` over duplicating logic in `src/cli/analyze.rs` or `src/cli/setup/steps.rs`.
- When setup validates a route that `analyze` will later use, health-check every configured tier that the real command depends on. For Scorpio that means both quick-thinking and deep-thinking model routes, not just one of them.
- Keep config file handling fail-closed for secret-bearing paths. If `HOME` is missing or invalid, error explicitly rather than guessing a fallback path.
- When bridging flat setup state into runtime config, prefer structured serialization over hand-built TOML strings or ad hoc table shaping.
- Preserve malformed user config by backup-and-recover rather than forcing manual cleanup first.
- Re-run the full CLI verification sequence after cross-cutting config fixes. This solved session passed:
  - `cargo fmt -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo nextest run --all-features --locked`
  - Final `nextest` result: `1148 passed, 3 skipped`

## Related Issues

- Related doc: `docs/solutions/best-practices/config-test-isolation-inline-toml-2026-04-11.md`
- Primary code areas: `src/config.rs`, `src/cli/analyze.rs`, `src/cli/setup/mod.rs`, `src/cli/setup/config_file.rs`, `src/cli/setup/steps.rs`
