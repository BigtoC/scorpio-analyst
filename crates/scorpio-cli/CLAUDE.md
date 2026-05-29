# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Scope: this file covers the **`scorpio-cli`** crate — the user-facing CLI layer. For workspace-wide build/test gates, behavioral guidelines, and the 5-phase analysis architecture, see the root `CLAUDE.md` and `docs/architecture/`. This crate contains *no domain logic*: it parses args, loads config, drives the setup wizard, and dispatches into `scorpio-core` (pipeline, config, storage) and `scorpio-reporters` (rendering).

## Commands

Run from the repo root (the package is `scorpio-cli`; the built binary and clap `bin_name` are `scorpio`):

```bash
cargo run -p scorpio-cli -- analyze AAPL          # Full 5-phase analysis
cargo run -p scorpio-cli -- setup                 # Interactive config wizard
cargo run -p scorpio-cli -- report list           # List past executions
cargo run -p scorpio-cli -- report show <ID>      # Show one stored report
cargo run -p scorpio-cli -- upgrade               # Self-update from GitHub releases

cargo nextest run -p scorpio-cli --all-features    # Run this crate's tests (CI uses nextest)
cargo nextest run -p scorpio-cli -E 'test(update)' # Run a single test / filter by name
```

`--all-features` matters: the `test-helpers` feature here is a pure forwarder to `scorpio-core/test-helpers`, which gates helpers some tests depend on.

## Architecture

### Entry flow (`main.rs`)
The boot sequence is async (`#[tokio::main]`) but most subcommands are synchronous, so they are bridged via `spawn_blocking` — `analyze` and `setup` build/enter their own runtimes inside the blocking task; nesting tokio runtimes directly would panic. Order: init tracing/Langfuse guard → `Cli::parse()` → spawn a **non-blocking background update check** (unless `--no-update-check` / `SCORPIO_NO_UPDATE_CHECK`) → dispatch → explicit Langfuse span flush before exit (Drop is bypassed on error exits). Exit code 1 on error.

The update-notice timing is deliberate: for `analyze`, the figlet banner prints, then waits up to a **500 ms grace window** for the background check so a notice can appear *before* the long pipeline run (user can Ctrl-C and upgrade first); other commands show the notice *after* completing. Don't collapse this into a single blocking check — it would stall fast subcommands.

### Module map (`src/cli/`)
- `mod.rs` — clap derive structs: `Cli` (+ global `--no-update-check`) and `Commands` enum (`Analyze`, `Setup`, `Upgrade`, `Report`).
- `analyze.rs` — loads `Config`, validates the symbol via core, builds a **multi-thread** runtime (so reporter file I/O doesn't block message rendering), runs `AnalysisRuntime::new(cfg).await.run(&symbol, execution_id)`, then composes a `ReporterChain` and runs reporters concurrently. A per-run `execution_id` (`Uuid::new_v4()`) ties the run to tracing and the snapshot store.
- `report.rs` — queries the core snapshot store for `list`/`show`; renders JSON (`--json`) or terminal output. Handles incomplete runs and schema-version mismatches gracefully.
- `update.rs` — `scorpio upgrade` + the background check: GitHub release feed, semver comparison, SHA256 verification, archive extraction, self-replace. A `MockUpdater` trait impl lets the large test suite exercise the flow without network.
- `setup/` — the interactive wizard (see below).

### Setup wizard (`src/cli/setup/`)
`mod.rs` orchestrates a 7-step flow, each step cancellable (Ctrl-C/ESC): Finnhub key (required) → FRED (optional) → Alpha Vantage (optional) → LLM provider keys (≥1 required; Copilot runs an OAuth **device flow inline**, not deferred) → provider routing for quick- vs deep-thinking tiers (models discovered via `scorpio_core::providers::factory::discover_setup_models()`, with manual-entry fallback) → Langfuse (optional) → **health check** that probes the selected LLM tiers with a retry loop (`steps.rs`, `health_check.rs`, `model_selection.rs`).

Config lives at `~/.scorpio-analyst/config.toml`, read/written through `scorpio_core::settings` (`PartialConfig`, `load_user_config_at`, `save_user_config_at`). On a malformed TOML file the wizard **backs up the bad file and continues with defaults** rather than failing — preserve this recovery behavior. Empty input on a prompt preserves the existing saved value.

### Crate boundaries
- **`scorpio-core`** — `AnalysisRuntime`, `Config`/`settings`, snapshot store, symbol validation, providers, observability. All real logic lives here.
- **`scorpio-reporters`** — `ReporterChain`, `TerminalReporter`, `JsonReporter` (writes `~/.scorpio-analyst/reports/<execution-id>.json`, or `--output-dir`), `ReportContext`.
- **`scorpio-server`** — not referenced by this crate.

## Testing notes
Tests are colocated in each module (`#[cfg(test)]`), with one integration test, `tests/install_release_contract.rs`, asserting the release-archive shape consumed by `upgrade`. Tests that mutate process env vars must hold the module-level `ENV_LOCK` mutex — env is global, and parallel tests will race otherwise. `tempfile` isolates config/report paths; `proptest` covers semver edge cases in `update.rs`.
