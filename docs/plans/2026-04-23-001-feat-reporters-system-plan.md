---
title: "feat: Reporters System for `scorpio analyze`"
type: feat
status: active
date: 2026-04-23
---

# feat: Reporter System for `scorpio analyze`

## Overview

Introduce a **Reporter** abstraction so the `scorpio analyze <SYMBOL>` command can
emit results through multiple reporters in a single run: the default terminal
output plus file-based or future integration legs. A new
`scorpio-reporters` workspace crate gives output code its own home outside the
clap/setup/update surface in `scorpio-cli`, so iteration 1 pays the terminal
formatter extraction cost once.

Iteration 1 scope is deliberately narrow:

1. Create `crates/scorpio-reporters/`: `Reporter` trait, `ReportContext`, `ReporterChain`.
2. Move the existing terminal report (`crates/scorpio-cli/src/report/`) into it, preserving behavior.
3. Add `JsonReporter` (the first file-based leg) and the `--json` /
   `--no-terminal` / `--output-dir` flags.

Iteration 1 does **not** add Telegram/Slack/email deps or reporter-specific
Cargo feature flags yet; those arrive with the first optional integration that
actually needs them. `TerminalReporter` runs by default. `--no-terminal`
suppresses the analyze banner plus the terminal reporter and is only valid when
another reporter is enabled. Reporter failures are fail-soft once analysis
succeeds: the chain continues, warnings are logged, and the process exits
non-zero only if every requested reporter fails.

## Problem Frame

Today the final emit step is a single hardcoded `println!`:

```rust
// crates/scorpio-cli/src/cli/analyze.rs:61
println!("{}", crate::report::format_final_report(&state));
```

`TradingState` after that line is dropped. There is no seam for alternative
outputs, and three near-term needs push on this:

1. **JSON artifact export** — the user wants a machine-readable local artifact for audit and local tooling.
2. **Markdown export** — shareable human-readable artifacts (paste into docs, Notion, PRs).
3. **Push-to-messenger** — integrations like Telegram/Slack that require their own async HTTP clients and config keys.

Bolting each of these onto `analyze.rs` as an `if flags.json { ... } else if
flags.tg { ... }` ladder scales poorly and keeps output concerns coupled to
the rest of the CLI command surface. A dedicated reporters crate makes the
terminal formatter extraction a one-time move, keeps future outputs isolated
from clap/setup/update code, and gives optional integrations a clean home when
they eventually need feature-gated deps.

Existing terrain confirmed during exploration:

- `TradingState` at `crates/scorpio-core/src/state/trading_state.rs:82` already derives `Serialize + Deserialize`, so iteration 1 can wrap it in a lightweight JSON artifact envelope without per-field serializers.
- `crates/scorpio-cli/src/report/final_report.rs:12` is a pure `fn(&TradingState) -> String` — a clean lift-and-shift candidate.
- `crates/scorpio-cli/src/cli/mod.rs:34` already uses `clap` derive for `Commands::Analyze { symbol }`; flags slot in trivially.
- `async-trait`, `chrono`, `uuid`, `serde_json`, `tokio`, `tracing` are all existing workspace deps — iteration 1 adds zero third-party crates.

## Naming

- **Crate:** `scorpio-reporters`
- **Trait:** `Reporter` — extends the existing `report` module's naming, so "the terminal reporter", "the JSON reporter", "the Telegram reporter" all read naturally.
- **Registry:** `ReporterChain` — an ordered list of reporters to run at emit time.
- **Per-run metadata:** `ReportContext` — canonical symbol, finished-at timestamp, output dir.

"Plugin" is avoided deliberately: it implies runtime/dylib loading, which
this isn't. Reporters are compile-time `Box<dyn Reporter>` polymorphism with
Cargo features per optional integration.

## Target Architecture

```
crates/
├── scorpio-core/                      # unchanged
├── scorpio-reporters/                 # NEW
│   ├── Cargo.toml                     # workspace deps; optional integration features can land later
│   └── src/
│       ├── lib.rs                     # Reporter trait, ReportContext, ReporterChain
│       ├── terminal/                  # moved from scorpio-cli/src/report/
│       │   ├── mod.rs                 # TerminalReporter + pub(crate) format_final_report
│       │   ├── final_report.rs
│       │   ├── coverage.rs
│       │   ├── valuation.rs
│       │   └── provenance.rs
│       └── json.rs                    # JsonReporter (first new reporter)
└── scorpio-cli/
    ├── Cargo.toml                     # + scorpio-reporters = { workspace = true }
    └── src/
        ├── lib.rs                     # remove `pub mod report;`
        └── cli/
            ├── mod.rs                 # Commands::Analyze(AnalyzeArgs)
            └── analyze.rs             # build_reporter_chain() + chain.run_all()
```

Iteration 1 keeps Cargo simple:

- `scorpio-cli` depends on `scorpio-reporters` directly with no reporter-specific feature wiring yet.
- `terminal` and `json` are built-ins for this slice.
- The first optional integration reporter introduces its own `scorpio-reporters` feature and optional deps in that follow-on PR.

## Trait and Types

Located in `crates/scorpio-reporters/src/lib.rs`:

```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scorpio_core::state::TradingState;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ReportContext {
    pub symbol: String,
    pub finished_at: DateTime<Utc>,
    /// Directory where file reporters write. Defaults to
    /// ~/.scorpio-analyst/reports and is created on demand.
    pub output_dir: PathBuf,
}

#[async_trait]
pub trait Reporter: Send + Sync {
    /// Stable identifier for logs and error messages (e.g. "terminal", "json").
    fn name(&self) -> &'static str;

    /// Emit a report for the completed analysis run.
    async fn emit(
        &self,
        state: &TradingState,
        ctx: &ReportContext,
    ) -> anyhow::Result<()>;
}

pub struct ReporterChain {
    reporters: Vec<Box<dyn Reporter>>,
}

impl ReporterChain {
    pub fn new() -> Self { Self { reporters: Vec::new() } }

    pub fn push<R: Reporter + 'static>(&mut self, r: R) {
        self.reporters.push(Box::new(r));
    }

    pub fn len(&self) -> usize { self.reporters.len() }

    /// Run every reporter concurrently via `futures::future::join_all`.
    /// Fail-soft: a failing reporter logs a sanitized warning and the chain
    /// continues. Returns the count of failed reporters.
    pub async fn run_all(
        &self,
        state: &TradingState,
        ctx: &ReportContext,
    ) -> usize {
        let futs = self.reporters.iter().map(|r| async move {
            (r.name(), r.emit(state, ctx).await)
        });
        futures::future::join_all(futs)
            .await
            .into_iter()
            .filter(|(name, result)| {
                if let Err(e) = result {
                    tracing::warn!(reporter = name, error = %e, "reporter failed");
                    true
                } else {
                    false
                }
            })
            .count()
    }
}
```

### Design decisions baked in

- **Async trait via `async-trait`** — already a workspace dep. Future Telegram/webhook reporters do real async I/O without `block_on`; JSON reporter uses `tokio::fs` cleanly.
- **`&TradingState`** — immutable borrow. Reporters cannot mutate state, so sequential-or-parallel is a free choice later.
- **`Send + Sync` bounds** — required for `Box<dyn Reporter>` storage and future concurrent execution.
- **Narrow `ReportContext`** — iteration 1 only carries metadata reporters actually consume: canonical symbol, finish time, and output dir. Add more fields later only when a reporter needs them.
- **Parallel execution via `futures::future::join_all`** — all reporters start concurrently; wall-clock time is bounded by the slowest reporter rather than the sum. Since `TradingState` is borrowed as `&TradingState` and all reporters are `Send + Sync`, sharing the reference across the join is sound without cloning state. Terminal output ordering is non-deterministic when multiple reporters emit to stdout, but in practice only `TerminalReporter` does so.
- **Fail-soft at chain level** — once analysis succeeds, one broken reporter shouldn't erase successful outputs from other reporters. Each failure is logged as a sanitized warning with the reporter name; the caller exits non-zero only when every requested reporter fails.

## Built-in Reporters (Iteration 1)

### `TerminalReporter` — `crates/scorpio-reporters/src/terminal/mod.rs`

Moves the existing module wholesale from `crates/scorpio-cli/src/report/`. The
1027-line `final_report.rs` and its helpers (`coverage.rs`, `valuation.rs`,
`provenance.rs`) come across unchanged. `colored` and `comfy-table` deps shift
from the CLI crate to the reporters crate.

```rust
pub struct TerminalReporter;

#[async_trait]
impl Reporter for TerminalReporter {
    fn name(&self) -> &'static str { "terminal" }

    async fn emit(
        &self,
        state: &TradingState,
        _ctx: &ReportContext,
    ) -> anyhow::Result<()> {
        println!("{}", format_final_report(state));
        Ok(())
    }
}
```

`format_final_report` becomes `pub(crate)` — only `TerminalReporter` calls it.

### `JsonReporter` — `crates/scorpio-reporters/src/json.rs`

```rust
pub struct JsonReporter;

impl JsonReporter {
    fn filename(ctx: &ReportContext) -> PathBuf {
        let ts = ctx.finished_at.format("%Y%m%dT%H%M%SZ");
        ctx.output_dir.join(format!("{}-{}.json", ctx.symbol, ts))
    }
}

#[async_trait]
impl Reporter for JsonReporter {
    fn name(&self) -> &'static str { "json" }

    async fn emit(
        &self,
        state: &TradingState,
        ctx: &ReportContext,
    ) -> anyhow::Result<()> {
        let path = Self::filename(ctx);
        let body = serde_json::to_string_pretty(&JsonReport::from_state(state))
            .context("serialising JsonReport")?;
        tokio::fs::create_dir_all(&ctx.output_dir).await
            .with_context(|| format!("creating {}", ctx.output_dir.display()))?;
        tokio::fs::write(&path, body).await
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}
```

- Filename convention: `<SYMBOL>-<ISO8601-UTC>.json`, e.g. `AAPL-20260423T142301Z.json`.
- Explicit output paths (`--json ./foo.json`) are a deliberate non-goal for iteration 1 — users control location via `--output-dir`. If needed later, swap `JsonReporter` for `JsonReporter { path: Option<PathBuf> }`.
- `JsonReporter` writes a local artifact only. Iteration 1 deliberately does **not** promise a stable public schema for downstream integrations.
- The file payload should be a small envelope such as `JsonReport { schema_version, generated_at, trading_state }` so the artifact can declare its own version from day one without inventing a large DTO yet.
- Because `TradingState` includes debate history, evidence, and token/accounting details, treat the JSON file as a potentially sensitive local artifact. Do not log payload contents or embed file contents in error messages.

## CLI Surface

`crates/scorpio-cli/src/cli/mod.rs` changes from the positional-string variant to an `Args` struct:

```rust
#[derive(Debug, Subcommand)]
pub enum Commands {
    Analyze(AnalyzeArgs),
    Setup,
    Upgrade,
}

#[derive(Debug, Clone, Default, Args)]
pub struct AnalyzeArgs {
    /// Ticker symbol (e.g. AAPL).
    #[arg(value_name = "SYMBOL")]
    pub symbol: String,

    /// Suppress the analyze banner and terminal reporter.
    /// Requires another reporter such as --json.
    #[arg(long = "no-terminal")]
    pub no_terminal: bool,

    /// Also write a pretty-printed JSON artifact to --output-dir.
    #[arg(long)]
    pub json: bool,

    /// Directory for file-based reporters. Defaults to
    /// ~/.scorpio-analyst/reports and is created if missing.
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,
}
```

`crates/scorpio-cli/src/cli/analyze.rs` is rewritten to build + run the chain:

```rust
pub fn run(args: &AnalyzeArgs) -> anyhow::Result<()> {
    let cfg = load_analysis_config()?;
    let _ = validate_symbol(&args.symbol)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to initialize async runtime")?;

    runtime.block_on(async move {
        let chain = build_reporter_chain(args);
        anyhow::ensure!(chain.len() > 0, "at least one reporter must be enabled");
        let analysis = AnalysisRuntime::new(cfg).await?;
        let state = analysis.run(&args.symbol).await?;
        let ctx = ReportContext {
            symbol: state.asset_symbol.clone(),
            finished_at: Utc::now(),
            output_dir: resolve_reports_dir(args.output_dir.as_deref())?,
        };
        let failures = chain.run_all(&state, &ctx).await;
        if failures == chain.len() {
            anyhow::bail!("{failures} reporter(s) failed; see logs");
        }
        Ok(())
    })
}

fn build_reporter_chain(args: &AnalyzeArgs) -> ReporterChain {
    let mut chain = ReporterChain::new();
    if !args.no_terminal {
        chain.push(TerminalReporter);
    }
    if args.json {
        chain.push(JsonReporter);
    }
    chain
}
```

`resolve_reports_dir(None)` should mirror the existing `~/.scorpio-analyst/...`
path pattern already used for snapshots and return
`$HOME/.scorpio-analyst/reports`.

`crates/scorpio-cli/src/main.rs` updates its dispatch arm:

```rust
Commands::Analyze(args) => {
    let args = args.clone();
    tokio::task::spawn_blocking(move || scorpio_cli::cli::analyze::run(&args))
        .await
        .map_err(|e| anyhow::anyhow!("analyze task failed: {e}"))
        .and_then(|r| r)
}
```

`AnalyzeArgs` derives `Clone` so it crosses the `spawn_blocking` boundary. Its
`Default` impl in tests should point `output_dir` at a temp dir fixture instead
of assuming the process current working directory.

## Iteration 1 Non-Goals

Spelled out to prevent scope creep:

- No `--markdown`, `--tg`, `--slack`, `--email`, `--webhook` in iteration 1. Each lands in its own PR with its own feature flag.
- No explicit `--json <path>` / `--markdown <path>` overrides. Filename is auto-derived; users steer via `--output-dir`.
- No new config keys in `~/.scorpio-analyst/config.toml`. Terminal and JSON reporters don't need them. Telegram/Slack will add their own in their iterations.
- No stable external JSON contract yet. Iteration 1's JSON artifact is versioned and intentionally scoped as a local/export convenience, not an API guarantee for third-party integrations.
- No dynamic plugin loading (dylib, WASM). All reporters are compile-time.
- No TUI/GUI work. The trait is designed to be reusable by a future TUI, but no code in `scorpio-reporters` depends on the `scorpio-cli` crate or `clap` parsing.

## Critical Files

| Purpose                   | Path                                                                                            |
|---------------------------|-------------------------------------------------------------------------------------------------|
| New crate root            | `crates/scorpio-reporters/Cargo.toml` (NEW)                                                     |
| Trait + chain + context   | `crates/scorpio-reporters/src/lib.rs` (NEW)                                                     |
| Terminal reporter (moved) | `crates/scorpio-reporters/src/terminal/` (moved from `crates/scorpio-cli/src/report/`)          |
| JSON reporter             | `crates/scorpio-reporters/src/json.rs` (NEW)                                                    |
| CLI args struct           | `crates/scorpio-cli/src/cli/mod.rs` (edited — `Analyze` becomes `Analyze(AnalyzeArgs)`)         |
| Chain build + dispatch    | `crates/scorpio-cli/src/cli/analyze.rs` (edited — replace `println!` emit with `chain.run_all`) |
| CLI lib module list       | `crates/scorpio-cli/src/lib.rs` (edited — drop `pub mod report;`)                               |
| Main dispatch             | `crates/scorpio-cli/src/main.rs` (edited — destructure `AnalyzeArgs`)                           |
| Workspace members         | `Cargo.toml` (edited — add `"crates/scorpio-reporters"`)                                        |
| Workspace deps            | `Cargo.toml` (edited — add `scorpio-reporters`; move `colored` / `comfy-table` if needed)       |

## Reused Code and Utilities

- `scorpio_core::state::TradingState` at `crates/scorpio-core/src/state/trading_state.rs:82` — already `Serialize + Deserialize`.
- `scorpio_core::data::symbol::validate_symbol` — unchanged, still called from the CLI.
- `scorpio_core::app::AnalysisRuntime` — unchanged entry point.
- `async-trait`, `chrono`, `uuid`, `serde_json`, `tokio` — all existing workspace deps; no new crate additions needed for iteration 1.
- `tracing::error!` — already used project-wide for structured error logs.

## Output and Security Contract

- `--json` produces a local file artifact, not stdout JSON. That keeps iteration 1 compatible with the existing interactive analyze UX while still creating a machine-readable export.
- `--no-terminal` means "disable the analyze banner and `TerminalReporter`" only. It does not suppress warnings written to stderr.
- At least one reporter must be enabled. `--no-terminal` by itself is a usage error.
- File reporters default to `$HOME/.scorpio-analyst/reports`, matching the app-owned path style already used for snapshots. The directory is created on demand.
- Reporter failure logs must be sanitized: log reporter name plus a high-level error string only. Never log secrets, request payloads, response bodies, or serialized report contents.
- JSON artifacts should be treated as potentially sensitive local files because they contain the full analysis state, debate history, provenance, and token accounting. Do not describe them as safe to commit/share by default.
- Exit status is based on requested outputs, not individual leg perfection: if at least one requested reporter succeeds after analysis finishes, the command exits `0`; if every requested reporter fails, it exits non-zero.

## Migration Steps

1. Add `crates/scorpio-reporters/` (`Cargo.toml`, `src/lib.rs` skeleton). Register in root `Cargo.toml` `[workspace] members`.
2. Move `crates/scorpio-cli/src/report/` → `crates/scorpio-reporters/src/terminal/`. Update `mod.rs` to re-export `format_final_report` as `pub(crate)` and define `TerminalReporter`.
3. Shift `colored` and `comfy-table` deps from `scorpio-cli`'s `[dependencies]` to `scorpio-reporters`'.
4. Add `crates/scorpio-reporters/src/json.rs` with `JsonReporter` plus a small `JsonReport` envelope type (`schema_version`, `generated_at`, `trading_state`).
5. In `crates/scorpio-cli/`: add `scorpio-reporters = { workspace = true }` dep; drop `pub mod report;` from `lib.rs`; update `cli/mod.rs` to switch `Commands::Analyze` to `Analyze(AnalyzeArgs)`; validate that `--no-terminal` requires another reporter; set the default `output_dir` via a helper that resolves `$HOME/.scorpio-analyst/reports`; update `cli/analyze.rs` to the new `run(&AnalyzeArgs)` signature and chain dispatch; update `main.rs` destructure and gate `print_banner()` on `!args.no_terminal`.
6. Add a `resolve_reports_dir()` helper that mirrors the existing snapshot-path HOME resolution and returns an app-owned reports directory when `--output-dir` is omitted.
7. Update existing tests in `cli/analyze.rs` that call `run("AAPL")` → `run(&AnalyzeArgs { symbol: "AAPL".into(), output_dir: Some(temp_dir.path().into()), ..Default::default() })`.
8. Add targeted tests (below).

## Testing Strategy

New tests inside `crates/scorpio-reporters/`:

- `tests/chain.rs` — fail-soft contract: a reporter that returns `Err` doesn't prevent later reporters from running; `run_all` returns the correct failure count.
- `tests/json.rs` — round-trip: `JsonReporter` writes a valid JSON file; reading and `serde_json::from_str::<JsonReport>` yields the expected `schema_version` and a `trading_state` equivalent to the fixture. Use `tempfile::tempdir()` for `output_dir`.
- `tests/terminal.rs` — equivalence: compare `scorpio_cli::report::format_final_report(fixture)` output with `scorpio_reporters::terminal::format_final_report(fixture)` during the move, normalizing ANSI if needed. Drop the old-path side after the extraction lands.
- `tests/json.rs` (second case) — missing output dir is created automatically before the file write succeeds.
- `tests/chain.rs` (second case) — if one of two reporters succeeds, the caller-level exit contract would remain success; if all reporters fail, it becomes an error.

Existing tests in `crates/scorpio-cli/src/cli/analyze.rs` (the three
symbol-validation tests plus the config-missing tests) update to pass
`AnalyzeArgs` instead of `&str`. They still exercise the same guards.

Add new CLI tests for:

- `--no-terminal` alone is rejected as invalid usage.
- `--no-terminal --json` skips the pre-dispatch banner path.
- `--output-dir` omitted resolves to an app-owned reports directory instead of `.`.

Existing tests in `crates/scorpio-cli/tests/` (release-archive contract) are
untouched.

## Verification

End-to-end checks (to run after implementation):

1. `cargo fmt -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` — match CI gate.
2. `cargo nextest run --workspace --all-features --locked --no-fail-fast` — all existing + new tests green.
3. `cargo run -- analyze AAPL` — identical terminal output to pre-refactor.
4. `cargo run -- analyze AAPL --json` — terminal output plus `~/.scorpio-analyst/reports/AAPL-<timestamp>.json` written when `--output-dir` is omitted; file round-trips through `serde_json::from_str::<JsonReport>`.
5. `cargo run -- analyze AAPL --no-terminal --json --output-dir /tmp/reports` — no figlet banner and no terminal report; JSON lands under `/tmp/reports/`.
6. `cargo run -- analyze AAPL --no-terminal` — exits with a clap/usage error because no reporters are enabled.
7. `cargo run -- analyze AAPL --json --output-dir /nonexistent/dir` — directory is created automatically and JSON write succeeds.
8. `cargo run -- analyze --help` shows the new flags with correct help text, including the `--no-terminal` constraint and the default reports-dir behavior.
