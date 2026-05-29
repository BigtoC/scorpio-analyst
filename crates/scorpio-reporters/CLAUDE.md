# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Scope: this file covers the **`scorpio-reporters`** crate — the output/rendering layer. For workspace-wide build/test gates, behavioral guidelines, and the 5-phase analysis architecture, see the root `CLAUDE.md` and `docs/architecture/`. This is a **library-only** crate (no binary, no features): it turns a completed `scorpio_core::state::TradingState` into human-readable terminal output and JSON artifacts. It owns *presentation only* — all domain types and analysis logic live in `scorpio-core`. Consumed by `scorpio-cli` and `scorpio-server`.

## Commands

```bash
cargo nextest run -p scorpio-reporters                       # Run this crate's tests (CI uses nextest)
cargo nextest run -p scorpio-reporters -E 'test(etf)'        # Filter by test name
cargo nextest run -p scorpio-reporters --test terminal       # Run one integration test file
```

There are no features and no binary target, so `--all-features` is a no-op here.

## Architecture

### The reporter abstraction (`src/lib.rs`)
Three pieces define the whole contract:

- `ReportContext` — per-run metadata (`symbol`, `finished_at`, `output_dir: Option<PathBuf>`). `output_dir` is created on demand by file reporters.
- `Reporter` trait (`#[async_trait]`, `Send + Sync + 'static`) — one method `emit(&self, state: Arc<TradingState>, ctx: Arc<ReportContext>) -> anyhow::Result<()>` plus a stable `name()` used in logs.
- `ReporterChain` — `push`-registered `Vec<Box<dyn Reporter>>`. `run_all` spawns **each reporter as its own `tokio::spawn` task** (true parallelism), then awaits all. It is **fail-soft**: a failing or *panicking* reporter is logged via `tracing::warn!` and counted; the others are unaffected. It returns the **failure count** (`usize`), not a bool — callers (e.g. the CLI) decide whether "some failed" vs "all failed" is fatal.

Key consequences when editing: reporters run **concurrently and unordered**; never assume one reporter sees another's side effects. `state` and `ctx` are `Arc`-shared, so reporters get read-only access without cloning `TradingState`.

### Concrete reporters
- **`JsonReporter` (`src/json.rs`)** — writes `<SYMBOL>-<ISO8601-UTC>.json` (e.g. `AAPL-20260423T142301Z.json`) into `ctx.output_dir`. Creates the dir on demand; **hard-fails if the file already exists** (no silent overwrite); requires `output_dir` to be `Some`. File I/O runs on `spawn_blocking`. Payload is `JsonReport { schema_version: u32, generated_at, trading_state }`.
- **`TerminalReporter` (`src/terminal/mod.rs`)** — infallible; prints to stdout. The render path is a **pure function** `render_final_report(&TradingState) -> String`; the only side effect (`println!`) lives in `emit`. Also exposes `render_execution_list(&[ExecutionSummary]) -> String` for the CLI's `report list`.

### Terminal rendering (`src/terminal/`)
`final_report.rs` is the large orchestrator (~1.3k lines) that assembles ~15 ordered sections (header → executive summary → trader proposal → analyst evidence → enrichment → scenario valuation → ETF panel → coverage → provenance → debate → risk review → deterministic safety check → auditor review → token usage → disclaimer). Section renderers are split into submodules: `coverage.rs`, `valuation.rs`, `provenance.rs`, `etf.rs`.

Formatting uses `comfy-table` for tables and `colored` for status coloring (which degrades gracefully on non-TTY). **Sections degrade independently** — missing inputs render as "Unavailable" rather than panicking or aborting the report. Conditional sections: ETF panels render only when valuation resolves to `ScenarioValuation::Etf`; the auditor section is skipped when `AuditStatus` is `Disabled`/`Pending`; enrichment blocks are skipped when all fields are `None`/`NotConfigured`. The ETF panel switches columns by `RenderPolicy` (Rich/Narrow/Ascii).

When adding a section, follow the existing pattern: write into a `&mut String`, guard on optionality with an "Unavailable"-style fallback, and keep the function pure so it stays unit-testable without stdout.

## Schema versioning (important)
`JsonReport.schema_version` is currently **v2**. There is **no migration tooling** — consumers handle bumps explicitly. The v2 reshape (Phase 6) moved equity-only fields under `state.equity.*`. Any change to the serialized `TradingState` shape that affects JSON output must bump this version and update the `tests/json.rs` schema assertions. Coordinate with `scorpio-core`'s state schema version (see `design-decisions.md`).

## Testing notes
- Integration tests in `tests/`: `chain.rs` (concurrency, fail-soft, panic recovery, empty chain), `terminal.rs` (section ordering, missing-data degradation, ETF rendering), `json.rs` (filename format, dir creation, no-overwrite, v2 equity shape).
- Unit tests are colocated (`#[cfg(test)]`) in `final_report.rs`, `coverage.rs`, `provenance.rs` for private helpers (e.g. `first_sentence()` abbreviation handling, color/label helpers).
- `tempfile` isolates JSON output dirs. Because terminal rendering is a pure function, prefer asserting on `render_final_report(&state)`'s returned string rather than capturing stdout.
