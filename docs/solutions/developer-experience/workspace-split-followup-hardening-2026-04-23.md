---
title: Workspace Split Follow-Up Hardening
date: 2026-04-23
category: docs/solutions/developer-experience
module: scorpio-core/scorpio-cli workspace split
problem_type: developer_experience
component: development_workflow
severity: medium
applies_when:
  - after crate extraction or workspace restructuring
  - when CLI identity or top-level commands must remain stable
  - when README or examples may have drifted from current workspace behavior
  - when adding regression coverage for refactor follow-up findings
symptoms:
  - clap surfaced `scorpio-cli` instead of the expected `scorpio` command name
  - README commands and config notes no longer matched workspace-era behavior
  - examples and facade failure paths were under-protected after the split
related_components:
  - documentation
  - testing_framework
  - tooling
tags:
  - workspace-split
  - cli-contract
  - readme
  - examples
  - regression-tests
  - scorpio-core
  - scorpio-cli
---

# Workspace Split Follow-Up Hardening

## Context

The split from a single crate into `scorpio-core` and `scorpio-cli` landed the new workspace shape, but several repo-level contracts drifted afterward. The code still built, but the user-facing CLI identity, README instructions, example placement, and regression coverage no longer described the same workspace reality.

This was a developer-experience problem more than one isolated runtime defect: contributors could easily follow stale docs, copy the wrong crate path from examples, or miss user-visible regressions because the relevant checks lived across clap metadata, repository docs, workspace metadata, and facade tests.

## Guidance

Treat a crate split as a contract-boundary change, not just a file move. After the split, harden the public surface in four places:

### 1. Pin the public CLI identity explicitly

If the internal bin target name differs from the command users should run, set the clap identity directly instead of relying on Cargo metadata defaults.

```rust
#[derive(clap::Parser, Debug)]
#[command(name = "scorpio", bin_name = "scorpio", version, about)]
pub struct Cli {
    // ...
}
```

Back that up with help/version tests so a future bin rename or clap refactor does not leak `scorpio-cli` back into the user-facing surface.

### 2. Keep examples with the crate they exercise

When examples import `scorpio_core`, they belong in `crates/scorpio-core/examples/`, not the repo root. Co-locating them with the owning crate makes import drift visible and keeps the run instructions honest.

```rust
use scorpio_core::app::AnalysisRuntime;
```

### 3. Treat README workflow text as a contract

After the split, the verified commands became:

- `cargo run -- setup`
- `cargo run -- analyze AAPL`

The README also needed to state that repo-root `config.toml` is inert at runtime and that the live user config lives under `~/.scorpio-analyst/config.toml`. If those statements are important for using the repo correctly, they should be covered by tests instead of relying on manual review.

### 4. Add repo-level contract tests for split-boundary assumptions

Some regressions only show up when multiple surfaces drift apart at once. Add tests that assert:

- the workspace exposes one CLI binary and core-hosted examples
- the README build-from-source instructions still match the actual workspace flow
- facade guarantees remain true on failure paths, not just happy paths

Useful examples from this hardening pass:

```rust
assert!(help_output.contains("Usage: scorpio"));
assert!(!help_output.contains("scorpio-cli"));
```

```rust
assert!(
    error.to_string().contains("final_execution_status"),
    "pipeline completion without final status must fail"
);
```

## Why This Matters

Workspace refactors are unusually good at creating low-grade drift: each individual mismatch looks small, but together they make the repo harder to trust. A user sees one command name, a README says another thing, examples compile against an old crate path, and runtime guarantees exist only as assumptions.

Encoding those expectations as tests and explicit metadata turns the split into a stable boundary again. That protects both users of `scorpio` and future contributors working in `scorpio-core` or `scorpio-cli`.

## When to Apply

- After splitting a monolithic crate into multiple workspace members
- After renaming a CLI binary while preserving the public command name
- When README or example commands changed because code moved across crate boundaries
- When a facade such as `AnalysisRuntime` has important failure guarantees that are not yet regression-tested

## Examples

Before this follow-up hardening:

- help and version output leaked the internal target name `scorpio-cli`
- root examples still imported `scorpio_analyst`
- README instructions described stale pre-split behavior
- `AnalysisRuntime` had no regression coverage for snapshot-store init failure or missing `final_execution_status`
- no contract test asserted that workspace metadata, docs, and examples still agreed

After this hardening:

- clap explicitly renders the public command as `scorpio`
- examples live under `crates/scorpio-core/examples/` and import `scorpio_core`
- README documents the workspace-era setup and run flow
- contract tests cover workspace metadata, README commands, and facade failure paths
- fresh verification passed with:
  - `cargo fmt -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo nextest run --workspace --all-features --locked --no-fail-fast` (`1252 passed`, `3 skipped`)

## Related

- Related learning: `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
- Related learning: `docs/solutions/best-practices/config-test-isolation-inline-toml-2026-04-11.md`
- GitHub issue search skipped: `gh` is not installed in this environment.
