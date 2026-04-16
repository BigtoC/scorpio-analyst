---
title: "feat: Add CLI update check and scorpio upgrade command"
type: feat
status: active
date: 2026-04-16
deepened: 2026-04-16
origin: docs/superpowers/specs/2026-04-16-cli-update-check-and-upgrade-design.md
---

# feat: Add CLI update check and scorpio upgrade command

## Overview

Every `scorpio` invocation spawns a background task that checks GitHub Releases for a newer version. If one is found, a colored notice box is printed to stderr after the subcommand finishes. A new `scorpio upgrade` subcommand performs an in-place binary replacement. Both behaviors can be suppressed via `--no-update-check` or `SCORPIO_NO_UPDATE_CHECK=1`.

## Problem Frame

Users running `scorpio` have no way to know when a new release is available, and no in-CLI path to update without leaving the terminal. The spec mirrors the npm-style update UX: non-blocking background check, hard-to-miss notice, one-command upgrade.

## Requirements Trace

- R1. Every invocation silently checks GitHub Releases in a background task (non-blocking, zero added latency to the command itself)
- R2. If a newer version exists, print a colored Unicode-box notice to **stderr** after the subcommand completes
- R3. Network failures, JSON errors, rate-limit hits, and timeouts are silently swallowed — never block or fail the CLI
- R4. `--no-update-check` global flag **and** `SCORPIO_NO_UPDATE_CHECK=1` env var each independently suppress the check
- R5. `scorpio upgrade` performs in-place binary replacement via the `self_update` crate, showing download progress
- R6. `scorpio upgrade` prints current version, and either "Already up to date" or "Updated successfully: vX → vY"
- R7. Release assets must be named `{bin_name}-{target}.{archive_ext}` (e.g. `scorpio-aarch64-apple-darwin.tar.gz`) — CI must already produce these (see spec §Release Asset Naming Convention)

## Scope Boundaries

- No changes to `src/cli/analyze.rs`, `src/cli/setup/`, agents, pipeline, data, indicators, providers, or workflow modules
- No caching or persistence of the last-check timestamp (every invocation checks afresh)
- No scheduled or daemon-style polling — single background task per invocation
- No interactive confirmation before upgrade (direct replacement)

## Context & Research

### Relevant Code and Patterns

- `src/main.rs` — currently 16 lines, synchronous `fn main()`. Dispatch is a plain `match cli.command`.
- `src/cli/mod.rs` — `Cli` struct with a single `command: Commands` field; `Commands` enum with `Analyze` and `Setup` variants. No global flags exist yet.
- `src/cli/analyze.rs` — `pub fn run(symbol: &str) -> anyhow::Result<()>`. Builds its own `tokio::runtime::Builder::new_current_thread()` runtime internally; `block_on` is called there.
- `src/cli/setup/mod.rs` — same pattern: synchronous `pub fn run() -> anyhow::Result<()>` with its own runtime.
- `src/report/final_report.rs` — the only existing user of the `colored` crate. Established style: `.bold()`, `.yellow().bold()` for attention, `.green().bold()` / `.red().bold()` for states, `.dimmed()` for secondary text.
- `src/data/finnhub.rs` — pattern for `reqwest`-based JSON deserialization into typed structs.
- `src/observability.rs` — `SCORPIO_LOG_FORMAT` env var read via `std::env::var(...)` — the naming pattern for single-underscore direct env vars (`SCORPIO_<NAME>`).

### Institutional Learnings

- CLI suppression env vars follow `SCORPIO_<NAME>` (single underscore) for direct flags, not the `SCORPIO__SECTION__KEY` double-underscore config-crate convention (see `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`)
- Never hold `std::sync::Mutex` across `.await` — use `tokio::sync` primitives or `spawn_blocking` for blocking work

### External References

- `self_update` crate docs: https://docs.rs/self_update — v0.44+ required for the builder API used in the spec
- `semver` crate docs: https://docs.rs/semver — v1, for `Version::parse` and `<` comparison

## Key Technical Decisions

- **`#[tokio::main]` conversion**: Converting `main` to async enables `tokio::spawn` for the background check at the top level. `tokio = {version="1", features=["full"]}` already supports this — no Cargo change needed for the runtime.
- **`spawn_blocking` bridge for existing subcommands**: `analyze::run()` and `setup::run()` each build their own `new_current_thread()` runtime and call `block_on`. Calling them directly from an async context would panic ("cannot start a runtime from within a runtime"). Wrapping them in `tokio::task::spawn_blocking` runs them on the blocking thread pool where they are free to build their own runtimes. This preserves the spec's "untouched files" constraint without adding nested async complexity.
- **`self_update` is blocking**: The crate's API (`.get_latest_release()`, `.update()`) is synchronous. Both the background version check and `run_upgrade()` must wrap blocking `self_update` calls in `spawn_blocking`.
- **`try_recv()` for post-command notice, guarded for Upgrade**: After the subcommand returns, a non-blocking `try_recv()` on the oneshot is used. If the background task hasn't finished yet (fast commands), the notice is silently skipped — consistent with R3. The `try_recv()` + notice block must be **skipped entirely** when `Commands::Upgrade` was dispatched — otherwise a successful upgrade prints a stale "run `scorpio upgrade`" box directly below the "Updated successfully" line.
- **Clap `env =` attribute for suppression**: `#[arg(long, global = true, env = "SCORPIO_NO_UPDATE_CHECK")]` on the `no_update_check: bool` field handles both the `--no-update-check` flag and the env var in one declaration. No manual `std::env::var` call needed in update logic.
- **`semver` crate for comparison**: Add `semver = "1"`. Strip leading `v` from the GitHub tag before `Version::parse`. Any parse error → treat as no update available (swallow silently). Extract a pure `fn should_notify(current: &str, latest: &str) -> bool` from `check_latest_version` — this is the unit-testable comparison core.
- **`rustls` TLS for `self_update`**: The `self_update` crate defaults to OpenSSL-based TLS. reqwest 0.13 in this project uses `rustls-platform-verifier` (not an explicit `rustls-tls` feature, but rustls is the underlying TLS stack). Adding `rustls` to `self_update` keeps the TLS backend consistent and avoids introducing a native OpenSSL dependency on Linux CI runners.
- **`check_latest_version` must be total (never panic/propagate)**: The background task is fire-and-forget — its `JoinHandle` is never awaited. If it panics, tokio emits the panic message directly to stderr via the default panic hook, interleaving with CLI output. `check_latest_version` must convert every possible error — including `JoinError` from its inner `spawn_blocking` — to `None` via match or `unwrap_or`, never via `?` or `unwrap`. The rule: "return `None` on any error, without exception."
- **Trait seam for `run_upgrade` testability**: `self_update`'s builder constructs a real HTTP client and cannot be unit-tested without a network. Introduce a `trait Updater` (private to `update.rs`) with a single method for the blocking update operation. The real implementation wraps `self_update`; a mock implementation returns canned `Status` values. This pattern mirrors the existing `LlmAgent` wrapper in `src/providers/factory/agent_test_support.rs`.
- **`format_update_notice` instead of `print_update_notice`**: Return a `String` from the formatting function rather than writing to implicit stderr. The call site calls `eprintln!("{}", format_update_notice(...))`. This pattern matches `format_final_report` in `src/report/final_report.rs` and makes the box content fully unit-testable without stderr capture tricks.

## Open Questions

### Resolved During Planning

- **Can `analyze::run()` and `setup::run()` be called from async context?** No — they call `block_on` internally. Solution: wrap in `tokio::task::spawn_blocking` in `main.rs` (see Key Technical Decisions).
- **Does clap `env =` handle `SCORPIO_NO_UPDATE_CHECK=1`?** Yes — clap's bool env parsing treats any non-empty, non-`false`, non-`0` string as `true`.
- **Is `reqwest` needed separately for the version check?** No — `self_update` handles the GitHub API call internally via its own HTTP client.

### Deferred to Implementation

- Exact box width for `format_update_notice` — may want dynamic width based on version string length; resolve during implementation once actual string lengths are known.
- Whether `self_update::get_latest_release()` returns the tag with or without the leading `v` — confirm at implementation time and adjust the `semver` strip logic accordingly.
- The `JoinError` type from `spawn_blocking` when existing subcommands panic — decide at implementation time whether to surface the panic message or use a generic "internal error" message.
- Whether pre-release tags (e.g., `v0.3.0-beta.1`) should be offered as upgrades — if the spec is silent, default to skipping pre-releases (parse as lower than the equivalent release version) and document the decision as a test comment.

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
#[tokio::main] async fn main()
  │
  ├─ parse CLI (Cli::parse)
  │
  ├─ if !cli.no_update_check:
  │    spawn background task ──→ spawn_blocking { self_update.get_latest_release() }
  │                          ──→ compare semver
  │                          ──→ oneshot::send(Option<latest_version>)
  │
  ├─ dispatch subcommand:
  │    Analyze  → spawn_blocking { analyze::run(symbol) }.await
  │    Setup    → spawn_blocking { setup::run() }.await
  │    Upgrade  → run_upgrade().await          ← async, internally uses spawn_blocking
  │
  ├─ handle subcommand error (eprintln + exit 1)
  │
  └─ if !is_upgrade: try_recv() on oneshot:
       Some(latest) → eprintln!(format_update_notice(current, latest)) to stderr
       None / pending / error → silent skip
```

`src/cli/update.rs` structure:

```
// Pure semver comparison — the unit-testable core
fn should_notify(current: &str, latest_version: &str) -> bool
  └─ Version::parse both strings (latest_version from Release.version is already v-stripped)
  └─ return latest > current; any parse error → false

pub async fn check_latest_version() -> Option<String>
  └─ spawn_blocking { get_release_blocking() }  ← must use match/unwrap_or; never ?/unwrap
       └─ Update::configure().repo_owner/repo_name.bin_name("scorpio").build()
            .get_latest_release() → release.version  ← already v-stripped (no leading "v")
       └─ should_notify(current, release.version) → if true: Some(version.to_string()), else None
            ← return canonical semver string (via Version::to_string), not raw tag
       └─ any error at any step → None (total; never panics)

pub fn format_update_notice(current: &str, latest: &str) -> String
  └─ build Unicode box string using colored chaining
  └─ returns String; caller uses eprintln!("{}", ...) for stderr output
  └─ tests assert on string content; never on ANSI codes

trait Updater { fn perform(&self) -> Result<Status, Error>; }  ← private seam
struct GithubUpdater(self_update builder)
struct MockUpdater(canned Status)

pub async fn run_upgrade() -> anyhow::Result<()>
  └─ print current version
  └─ spawn_blocking { updater.perform() }
  └─ print "Already up to date (vX)" or "Updated successfully: vX → vY"
```

## Implementation Units

- [ ] **Unit 1: Add self_update and semver to Cargo.toml**

  **Goal:** Make `self_update` and `semver` available as dependencies.

  **Requirements:** R5, R1 (both units that use GitHub API depend on self_update)

  **Dependencies:** None

  **Files:**
  - Modify: `Cargo.toml`

  **Approach:**
  - Add `self_update = { version = "0.44", default-features = false, features = ["reqwest", "archive-tar", "compression-flate2", "archive-zip", "compression-zip-deflate", "rustls"] }` under `[dependencies]` — the `reqwest` feature is required to enable the HTTP client; `rustls` activates `reqwest?/rustls` on top of it. Without `reqwest`, all API calls silently fail at runtime.
  - Add `semver = "1"` under `[dependencies]`
  - Do not change the `[dev-dependencies]` block or any existing dependency entries

  **Patterns to follow:**
  - Existing dep block style in `Cargo.toml` (version strings, feature arrays)

  **Test scenarios:**
  - Test expectation: none — pure dependency configuration; verified by `cargo build` succeeding after Unit 3/4 are implemented

  **Verification:**
  - `cargo build` compiles without error after all four units are in place

---

- [ ] **Unit 2: Extend CLI structure in src/cli/mod.rs**

  **Goal:** Add the `Upgrade` command variant, `--no-update-check` global flag, and `pub mod update;` declaration.

  **Requirements:** R4, R5

  **Dependencies:** None (pure struct/enum changes, no logic)

  **Files:**
  - Modify: `src/cli/mod.rs`
  - Test: `tests/cli_structure_test.rs` (or inline `#[cfg(test)]` module in `src/cli/mod.rs`)

  **Approach:**
  - Add `pub mod update;` alongside existing `pub mod analyze;` and `pub mod setup;`
  - Add `no_update_check: bool` field to `Cli` with `#[arg(long, global = true, env = "SCORPIO_NO_UPDATE_CHECK")]` — add above `#[command(subcommand)]`
  - Add `/// Upgrade scorpio to the latest release from GitHub` doc comment and `Upgrade` variant to `Commands` enum

  **Patterns to follow:**
  - Existing `Analyze` and `Setup` variant style in `Commands`
  - Clap 4 derive attribute style used in the current `Cli` struct

  **Test scenarios:**
  - Happy path: parsing `["scorpio-analyst", "upgrade"]` produces `Commands::Upgrade`
  - Happy path: parsing `["scorpio-analyst", "--no-update-check", "analyze", "AAPL"]` sets `no_update_check = true`
  - Happy path: parsing `["scorpio-analyst", "analyze", "AAPL", "--no-update-check"]` (flag after subcommand) also sets `no_update_check = true` (clap `global = true` behavior)
  - Happy path: `SCORPIO_NO_UPDATE_CHECK=1` env var sets `no_update_check = true` without passing the flag
  - Edge case: `SCORPIO_NO_UPDATE_CHECK=0` leaves `no_update_check = false`
  - Edge case: `SCORPIO_NO_UPDATE_CHECK=false` leaves `no_update_check = false`

  **Verification:**
  - `cargo clippy --all-targets -- -D warnings` passes
  - `scorpio upgrade --help` outputs the subcommand help text (manual check or integration test)

---

- [ ] **Unit 3: Implement src/cli/update.rs**

  **Goal:** Implement the public API for update checking and upgrades: `check_latest_version`, `format_update_notice`, and `run_upgrade`, plus the private `should_notify` comparison core and `Updater` trait seam.

  **Requirements:** R1, R2, R3, R5, R6

  **Dependencies:** Unit 1 (self_update + semver crates), Unit 2 (pub mod update declaration)

  **Files:**
  - Create: `src/cli/update.rs`
  - Test: inline `#[cfg(test)]` module in `src/cli/update.rs`

  **Approach:**
  - `fn should_notify(current: &str, latest_version: &str) -> bool` — pure private fn; `latest_version` is already v-stripped (from `Release.version`); calls `semver::Version::parse` on both strings; returns `latest > current`; any parse error → `false` (never panics). The v-strip in the original spec sketch is a defensive no-op when using `Release.version` directly.
  - `async fn check_latest_version() -> Option<String>` — wraps blocking `self_update` `get_latest_release()` in `spawn_blocking`; uses `match`/`unwrap_or` everywhere (never `?` or `unwrap`) so all errors — including `JoinError` from an inner panic — convert to `None`; calls `should_notify`; if notifiable, returns `Some(version.to_string())` using the canonical parsed semver string (not the raw tag) to prevent ANSI-injected or malformed tags from reaching `format_update_notice`. Define a named constant `UPDATE_CHECK_TIMEOUT_SECS: u64 = 5` (matching the `HEALTH_CHECK_TIMEOUT_SECS` pattern in `src/constants.rs`) and apply a wall-clock timeout to the `get_latest_release()` call; on timeout, return `None` per R3.
  - `GithubUpdater`: set `.bin_name("scorpio")` — the release CI stages the binary as `scorpio` inside archives, not `scorpio-analyst` (the Cargo package name). Also set `.repo_owner("BigtoC").repo_name("scorpio-analyst")` as compile-time string literals (not user-configurable).
  - `fn format_update_notice(current: &str, latest: &str) -> String` — pure fn returning a formatted String; caller emits with `eprintln!`; uses `colored` chaining for yellow/bold text; box characters: `╭`, `─`, `╮`, `│`, `╰`, `╯`; does not write to stderr directly (matches `format_final_report` pattern)
  - `trait Updater` — private seam with a single method for the blocking update operation; `GithubUpdater` wraps the real `self_update` builder; `MockUpdater` accepts canned `Status` values for tests
  - `async fn run_upgrade() -> anyhow::Result<()>` — prints current version; calls `spawn_blocking { updater.perform() }`, mapping `JoinError` to `anyhow::Error` via `.map_err`; prints "Already up to date (vX.Y.Z)" or "Updated successfully: vX → vY"; all error paths use `?` with `anyhow` context strings

  **Patterns to follow:**
  - `src/report/final_report.rs` — `colored` import/chaining style; `format_final_report` returns String (same contract as `format_update_notice`)
  - `src/providers/factory/agent_test_support.rs` — trait wrapper pattern for the `Updater` seam
  - `src/cli/analyze.rs` — `anyhow::Result<()>` return type, `{e:#}` error formatting

  **Test scenarios:**
  - `should_notify` (pure fn, no tokio needed):
    - Happy path: `("0.2.0", "v0.3.0")` → `true`
    - Happy path: `("0.3.0", "v0.3.0")` (equal) → `false`
    - Edge case: `("0.3.1", "v0.3.0")` (current is newer than release) → `false`
    - Edge case: `("0.3.0", "v0.3.0-beta.1")` (pre-release is lower than release per semver) → `false`
    - Error path: `("0.2.0", "not-semver")` → `false` (no panic)
    - Error path: `("0.2.0", "")` → `false` (no panic)
    - Property (proptest): `should_notify(v, v)` is always `false` for any valid version string
  - `format_update_notice` (pure fn, no tokio):
    - Happy path: `("0.2.1", "0.3.0")` → output contains both version strings, "scorpio upgrade", and box border characters `╭`/`╰`/`│`
    - Defensive: `("0.2.1", "0.2.1")` (called with equal versions — shouldn't happen but must not panic) → non-empty output, no panic
    - Property (proptest): any string inputs → no panic, output is non-empty
    - Do not assert on ANSI escape codes — test text content only
  - `run_upgrade` via `MockUpdater` (unit tests):
    - Happy path: mock returns `Status::UpToDate("0.2.1")` → `Ok(())`, output contains "already up to date" (case-insensitive)
    - Happy path: mock returns `Status::Updated("0.3.0")` → `Ok(())`, output contains "updated successfully" and `"0.3.0"`
    - Error path: mock returns `Err(network_error)` → `Err` returned; error message is non-empty and includes context (not just "error")
    - Error path: inner `spawn_blocking` panics → `JoinError` is mapped to `anyhow::Error`, not re-panicked; `run_upgrade` returns `Err`
    - Error path: `Err` wrapping a permission-denied IO error → error propagates; output does not contain "successfully"

  **Verification:**
  - Unit tests in `#[cfg(test)]` pass under `cargo nextest run`
  - `cargo clippy --all-targets -- -D warnings` passes (no unused imports — `colored::Colorize` used, `self_update` types used, `semver` used)

---

- [ ] **Unit 4: Convert src/main.rs to async and wire update check**

  **Goal:** Convert `main` to `#[tokio::main] async fn main()`, spawn the background version check, dispatch existing subcommands via `spawn_blocking`, dispatch `Upgrade`, and print the update notice after dispatch.

  **Requirements:** R1, R2, R3, R4

  **Dependencies:** Unit 2 (Cli has `no_update_check`), Unit 3 (update functions exist)

  **Files:**
  - Modify: `src/main.rs`
  - Test: inline `#[cfg(test)]` or integration test in `tests/` for suppression behavior

  **Approach:**
  - Replace `fn main()` with `#[tokio::main] async fn main()`
  - **Immediately after `Cli::parse()`**, extract `let is_upgrade = matches!(cli.command, Commands::Upgrade);` — this boolean must be captured before the dispatch `match` consumes `cli.command` by move; reading `cli.command` again after the match will not compile
  - Spawn background check (gated on `!cli.no_update_check`): create a `tokio::sync::oneshot` channel; `tokio::spawn(async move { let result = check_latest_version().await; let _ = tx.send(result); })`
  - Dispatch block — `Commands::Analyze` and `Commands::Setup` wrapped in `tokio::task::spawn_blocking`; `Commands::Upgrade` calls `cli::update::run_upgrade().await` directly (natively async, no `spawn_blocking`)
  - After dispatch on **success only**: check `if !is_upgrade` before calling `try_recv()` — the notice must not appear after a successful upgrade (would tell the user to run `scorpio upgrade` immediately after they just did); on dispatch error, `process::exit(1)` is called immediately and the notice block is never reached
  - Notice block: `if let Ok(Some(latest)) = rx.try_recv() { eprintln!("{}", format_update_notice(current, &latest)); }`
  - Error handling: same `eprintln!("{e:#}") + std::process::exit(1)` pattern; `JoinError` from `spawn_blocking` mapped to `anyhow::Error` via `.map_err`
  - `current` version for the notice: `env!("CARGO_PKG_VERSION")` (compile-time constant)
  - Extract `fn try_show_update_notice(rx: oneshot::Receiver<Option<String>>, current: &str)` as a free function — makes the oneshot/notice logic unit-testable independent of the full async main

  **Patterns to follow:**
  - Current error-handling pattern in `src/main.rs` (eprintln + exit 1)
  - `tokio::task::spawn_blocking` pattern from `src/cli/analyze.rs` internal runtime construction

  **Test scenarios:**
  - `try_show_update_notice` (unit tests, uses `#[tokio::test]`):
    - Happy path: sender sends `Some("0.3.0")` before call → notice string contains both version strings
    - Happy path: sender sends `None` (up-to-date) → no output
    - Edge case: sender not yet sent (task still in flight) → `try_recv()` returns `TryRecvError::Empty` → no output (this is the intentional best-effort contract; document with a comment)
    - Edge case: sender dropped without sending (background task panicked) → `try_recv()` returns `TryRecvError::Disconnected` → no output, no panic
  - Suppression (CLI-level, via clap):
    - Happy path: `--no-update-check` flag → oneshot is never created, notice is never printed
    - Happy path: `SCORPIO_NO_UPDATE_CHECK=1` env var → same suppression
  - Upgrade guard:
    - Integration: dispatching `Commands::Upgrade` (mock upgrade succeeds) → notice block is skipped; no "run `scorpio upgrade`" box printed after "Updated successfully"
  - Error path: subcommand returns `Err` → error printed + exit 1, `try_show_update_notice` is never reached
  - Integration: existing `analyze` and `setup` subcommands return correctly after `spawn_blocking` wrapping

  **Verification:**
  - `cargo build` succeeds
  - `cargo nextest run --all-features --locked` passes
  - `cargo clippy --all-targets -- -D warnings` passes
  - Manual: `scorpio analyze AAPL --no-update-check` runs without notice
  - Manual: `scorpio upgrade` triggers the download flow

## System-Wide Impact

- **main.rs runtime change**: All existing subcommands now run inside a `spawn_blocking` thread. The `JoinError` error type from `spawn_blocking` is new — it must be mapped to `anyhow::Error` via `.map_err`, not via `unwrap`.
- **Upgrade command notice ordering**: The `try_recv()` + notice block must be gated behind `!matches!(cli.command, Commands::Upgrade)`. Without this guard, a successful upgrade prints a "run `scorpio upgrade`" box immediately after "Updated successfully" — logically contradictory output in the primary upgrade success path.
- **Background task panic surface — `tokio::spawn`, not `JoinError`**: The fire-and-forget `tokio::spawn` for the version check is never awaited. If it panics, tokio emits the panic message directly to stderr via its default panic hook — not through `JoinError`. The plan's `JoinError` mapping applies only to the `spawn_blocking` subcommand dispatch. See Key Technical Decisions for the complete `check_latest_version` error-absorption rule.
- **SIGINT non-cancellability of `spawn_blocking`**: With `#[tokio::main]` multi-thread runtime, pressing Ctrl+C during a long `analyze` run sends a signal to the tokio runtime but `spawn_blocking` tasks run to completion regardless. An analysis with `analyst_timeout_secs = 3000` (50 min) will appear unresponsive to Ctrl+C. This is a behavioral change from the current synchronous design (where SIGINT terminates immediately). Acceptable as-is for this plan — document the limitation in the command's help text or release notes.
- **`process::exit(1)` async drop semantics**: The pre-existing `process::exit(1)` call bypasses async drop of `SnapshotStore` (SQLite via sqlx) and Copilot child processes. This is not a regression introduced by this plan — the same bypass exists in the current synchronous design. Preserved unchanged.
- **Subcommand parity**: The `Upgrade` command does NOT go through `spawn_blocking` — it is natively async. The dispatch match arm must treat it separately from the existing sync subcommands.
- **stderr cleanliness**: `format_update_notice` returns a String that the caller emits with `eprintln!`. Piped stdout (e.g., `scorpio analyze AAPL | jq`) must remain clean. Any accidental `println!` in update logic would break this.
- **CI asset naming**: The release CI workflow (`.github/workflows/` — not modified by this plan) must produce assets matching `scorpio-{target}.{tar.gz|zip}`. This is a runtime contract required for `scorpio upgrade` to work. Verify naming alignment with the existing `install.sh` targets before shipping the `Upgrade` command.
- **Unchanged invariants**: `analyze::run()` and `setup::run()` function signatures, behavior, and internal runtime patterns are entirely unchanged — they remain synchronous public functions returning `anyhow::Result<()>`.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| `spawn_blocking` wrapping `analyze::run()` adds an extra OS thread hop per invocation | Accepted — latency difference is sub-millisecond for thread acquisition; the existing current-thread runtime inside `analyze::run()` dominates |
| `self_update` pulls in additional transitive dependencies (reqwest, flate2) that conflict with existing versions | Check `cargo tree` after Unit 1; resolve any version conflicts before proceeding to Unit 3 |
| `self_update` v0.44 API shape differs from the spec's code sketch (e.g., `UpdateStatus` variant names, `get_latest_release` return type) | Verify builder method names, `Status` enum variants, and whether the tag includes `v` prefix against actual crate docs before writing Unit 3 |
| SIGINT during `analyze` appears unresponsive after `spawn_blocking` conversion | Accepted for this plan; document limitation in release notes; a future cancellation pass can add a signal handler that calls `process::exit` directly |
| Missing `rustls` feature causes OpenSSL link errors on Linux CI | Covered — `rustls` feature is included in Unit 1 |
| Release assets not yet named to `self_update` convention | Out of scope for this plan; flag to CI owner before shipping the `Upgrade` command to production |
| Background check adds DNS/network overhead on every invocation for users with poor connectivity | Mitigated by async non-blocking design — the subcommand runs in parallel; only `try_recv()` at the end adds overhead, and that is O(1) |

## Sources & References

- **Origin document:** [docs/superpowers/specs/2026-04-16-cli-update-check-and-upgrade-design.md](docs/superpowers/specs/2026-04-16-cli-update-check-and-upgrade-design.md)
- Related code: `src/main.rs`, `src/cli/mod.rs`, `src/cli/analyze.rs`, `src/cli/setup/mod.rs`, `src/report/final_report.rs`
- Institutional learning: `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
