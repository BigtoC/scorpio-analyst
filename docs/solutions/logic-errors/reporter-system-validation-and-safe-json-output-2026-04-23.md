---
title: Reporter System Validation and Safe JSON Output
date: 2026-04-23
category: docs/solutions/logic-errors
module: reporter-system
problem_type: logic_error
component: tooling
symptoms:
  - Terminal-only `analyze` runs failed in `HOME`-less environments even when JSON output was not enabled.
  - `--no-terminal` and `--output-dir` were accepted without `--json`, allowing invalid reporter configurations.
  - Reporter argument validation happened after config and runtime setup instead of failing fast.
  - JSON report filenames could collide and overwrite existing artifacts.
  - Planned reporter parity, banner, and failure-path tests were missing.
root_cause: logic_error
resolution_type: code_fix
severity: high
related_components:
  - scorpio-cli
  - scorpio-reporters
  - testing_framework
tags:
  - reporter-system
  - feature-reporter-crate
  - cli-validation
  - json-reporting
  - collision-safety
  - homeless-terminal
  - regression-tests
---

# Reporter System Validation and Safe JSON Output

## Problem

The reporter-system work on `feature/reporter-crate` mixed terminal and file-output assumptions, so the CLI contract and the runtime contract drifted apart. Terminal-only runs could still depend on filesystem state, invalid reporter flag combinations were accepted too long, and JSON artifact writes were not collision-safe.

## Symptoms

- `scorpio analyze <SYMBOL>` could fail in terminal-only mode when `HOME` was unset because `crates/scorpio-cli/src/cli/analyze.rs` resolved a report directory even when no file reporter was enabled.
- `--no-terminal` without `--json` and `--output-dir` without `--json` were accepted past clap parsing and only rejected later in execution.
- Repeated JSON writes for the same symbol in the same second could overwrite an earlier file in `crates/scorpio-reporters/src/json.rs`.
- The reporter plan promised parity, banner, and failure-path regression coverage that had not yet been added.

## What Didn't Work

- Modeling `scorpio_reporters::ReportContext.output_dir` as a required `PathBuf` forced every reporter mode to look like it needed a writable output directory.
- Relying only on runtime validation inside `run()` was too late for obvious invalid flag combinations that clap could reject earlier.
- Using second-level timestamps as the only uniqueness guard made JSON artifact naming probabilistic instead of safe.
- Leaving terminal rendering private made the final report path harder to test directly, which contributed to the missing parity coverage.

## Solution

The fix tightened the reporter contract in both the shared runtime context and the CLI surface, then closed the missing regression gaps.

### 1. Make output directories optional for terminal-only runs

`crates/scorpio-reporters/src/lib.rs` changed `ReportContext.output_dir` from `PathBuf` to `Option<PathBuf>` so only file reporters depend on a resolved directory:

```rust
pub struct ReportContext {
    pub symbol: String,
    pub output_dir: Option<PathBuf>,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}
```

`crates/scorpio-cli/src/cli/analyze.rs` now resolves an output directory only when JSON reporting is enabled.

### 2. Reject invalid reporter flag combinations at the clap boundary

`crates/scorpio-cli/src/cli/mod.rs` now couples the reporter flags directly to `--json`:

```rust
#[arg(long = "no-terminal", requires = "json")]
pub no_terminal: bool,

#[arg(long, value_name = "DIR", requires = "json")]
pub output_dir: Option<PathBuf>,
```

This blocks invalid invocations before config loading or runtime startup.

### 3. Keep a semantic guard at command entry

`crates/scorpio-cli/src/cli/analyze.rs` still performs explicit reporter validation at the start of `run()` so non-clap callers fail consistently:

```rust
validate_reporter_args(args)?;
```

The guard enforces the two key contract errors:

```rust
anyhow::bail!("at least one reporter must be enabled; use --json if --no-terminal is set");
anyhow::bail!("--output-dir requires --json");
```

### 4. Make JSON artifact creation collision-safe

`crates/scorpio-reporters/src/json.rs` now combines millisecond timestamps with exclusive file creation:

```rust
let timestamp = ctx.finished_at.format("%Y%m%dT%H%M%S%3fZ");

OpenOptions::new()
    .write(true)
    .create_new(true)
```

Millisecond precision reduces accidental reuse, and `create_new(true)` turns uniqueness into an enforced filesystem guarantee. A collision now fails safely instead of overwriting an existing report.

### 5. Add the missing reporter regression coverage

This pass added or updated tests for three themes:

- CLI contract checks for invalid reporter flag combinations and early validation
- terminal behavior checks for `HOME`-less execution and banner suppression
- reporter safety checks for JSON collisions, panic isolation, and terminal report parity

## Why This Works

The reporter contract is now consistent across the shared context, the clap surface, and the runtime entrypoint. Terminal mode no longer inherits file-output requirements, invalid reporter flag combinations fail before expensive setup, and JSON artifacts cannot silently overwrite an existing file.

## Prevention

- Keep optional runtime features aligned with optional resources. Terminal-only paths should not require filesystem setup.
- Reject invalid CLI flag combinations in clap metadata and keep a small semantic guard at command entry for defense in depth.
- Protect generated artifacts with exclusive create semantics instead of timestamp-only naming.
- Treat plan-promised parity and failure-path tests as part of the implementation contract.
- Re-run the full repo verification sequence after cross-cutting CLI or reporter changes. This fix passed:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast`
  - Final `nextest` result: `1274 passed`, `3 skipped`

## Related Issues

- Plan: `docs/plans/2026-04-23-001-feat-reporters-system-plan.md`
- Related learning: `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
- Related learning: `docs/solutions/developer-experience/workspace-split-followup-hardening-2026-04-23.md`
