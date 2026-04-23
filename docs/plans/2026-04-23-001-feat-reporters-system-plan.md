---
title: "feat: Reporters System for `scorpio analyze`"
type: feat
status: active
date: 2026-04-23
---

# feat: Reporter Plugin System for `scorpio analyze`

## Overview

Introduce a **Reporter** abstraction so the `scorpio analyze <SYMBOL>` command can
emit results through multiple "legs" in a single run: the default terminal
report plus any combination of `--json`, `--markdown`, `--tg`, and future
flags. A new `scorpio-reporters` workspace crate hosts the trait, registry,
and all built-in reporters; heavy per-integration deps (Telegram, Slack,
email) live behind feature flags so the CLI binary only pays for what it
ships.

Iteration 1 scope is deliberately narrow:

1. Create `crates/scorpio-reporters/`: `Reporter` trait, `ReportContext`, `ReporterChain`.
2. Move the existing terminal report (`crates/scorpio-cli/src/report/`) into it, preserving behavior.
3. Add `JsonReporter` (the first non-stdout leg) and the `--json` / `--no-terminal` / `--output-dir` flags.

Flags are additive: stdout always runs unless `--no-terminal` is passed.
Failures are fail-soft per reporter — one broken leg logs and the chain
continues.

## Problem Frame

Today the final emit step is a single hardcoded `println!`:

```rust
// crates/scorpio-cli/src/cli/analyze.rs:61
println!("{}", crate::report::format_final_report(&state));
```

`TradingState` after that line is dropped. There is no seam for alternative
outputs, and three real near-term needs push on this:

1. **JSON export** — the user wants machine-readable output for downstream tooling and audit.
2. **Markdown export** — shareable human-readable artifacts (paste into docs, Notion, PRs).
3. **Push-to-messenger** — integrations like Telegram/Slack that require their own async HTTP clients and config keys.

Bolting each of these onto `analyze.rs` as an `if flags.json { ... } else if
flags.tg { ... }` ladder scales poorly and drags unrelated deps
(`reqwest`/`lettre`/…) into `scorpio-cli` permanently. A dedicated reporters
crate with feature-gated modules keeps the CLI binary lean and lets each new
output land in its own PR without touching the dispatch code.

Existing terrain confirmed during exploration:

- `TradingState` at `crates/scorpio-core/src/state/trading_state.rs:82` already derives `Serialize + Deserialize`; no custom serializer needed.
- `crates/scorpio-cli/src/report/final_report.rs:12` is a pure `fn(&TradingState) -> String` — a clean lift-and-shift candidate.
- `crates/scorpio-cli/src/cli/mod.rs:34` already uses `clap` derive for `Commands::Analyze { symbol }`; flags slot in trivially.
- `async-trait`, `chrono`, `uuid`, `serde_json`, `tokio`, `tracing` are all existing workspace deps — iteration 1 adds zero third-party crates.

## Naming

- **Crate:** `scorpio-reporters`
- **Trait:** `Reporter` — extends the existing `report` module's naming, so "the terminal reporter", "the JSON reporter", "the Telegram reporter" all read naturally.
- **Registry:** `ReporterChain` — an ordered list of reporters to run at emit time.
- **Per-run metadata:** `ReportContext` — symbol, run id, timing, output dir.

"Plugin" is avoided deliberately: it implies runtime/dylib loading, which
this isn't. Reporters are compile-time `Box<dyn Reporter>` polymorphism with
Cargo features per optional integration.

## Target Architecture

```
crates/
├── scorpio-core/                      # unchanged
├── scorpio-reporters/                 # NEW
│   ├── Cargo.toml                     # workspace deps; features per integration
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

## Trait and Types

Located in `crates/scorpio-reporters/src/lib.rs`:

```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scorpio_core::state::TradingState;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ReportContext {
    pub symbol: String,
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    /// Directory where file reporters write; resolved from --output-dir.
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

    /// Run every reporter sequentially in insertion order.
    /// Fail-soft: a failing reporter is logged and the chain continues.
    /// Returns the count of failed reporters (caller decides exit code).
    pub async fn run_all(
        &self,
        state: &TradingState,
        ctx: &ReportContext,
    ) -> usize {
        let mut failures = 0;
        for r in &self.reporters {
            if let Err(e) = r.emit(state, ctx).await {
                tracing::error!(reporter = r.name(), error = ?e, "reporter failed");
                failures += 1;
            }
        }
        failures
    }
}
```

### Design decisions baked in

- **Async trait via `async-trait`** — already a workspace dep. Future Telegram/webhook reporters do real async I/O without `block_on`; JSON reporter uses `tokio::fs` cleanly.
- **`&TradingState`** — immutable borrow. Reporters cannot mutate state, so sequential-or-parallel is a free choice later.
- **`Send + Sync` bounds** — required for `Box<dyn Reporter>` storage and future concurrent execution.
- **`ReportContext`** — out-of-band metadata (symbol, timing, `run_id`, `output_dir`) lives here so each reporter doesn't re-derive it. `run_id` lets reporters correlate a single run across multiple legs.
- **Sequential execution in insertion order** — deterministic, preserves "terminal prints first" UX. If later we need parallel, it's a one-method change (`run_all_concurrent`).
- **Fail-soft at chain level** — one broken reporter (e.g. Telegram network failure) shouldn't kill the stdout report we already printed. Each failure is logged via `tracing::error!` with the reporter name; the caller inspects the failure count to set the process exit code.

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
        let body = serde_json::to_string_pretty(state)
            .context("serialising TradingState to JSON")?;
        tokio::fs::write(&path, body).await
            .with_context(|| format!("writing {}", path.display()))?;
        println!("✓ JSON report written to {}", path.display());
        Ok(())
    }
}
```

- Filename convention: `<SYMBOL>-<ISO8601-UTC>.json`, e.g. `AAPL-20260423T142301Z.json`.
- Explicit output paths (`--json ./foo.json`) are a deliberate non-goal for iteration 1 — users control location via `--output-dir`. If needed later, swap `JsonReporter` for `JsonReporter { path: Option<PathBuf> }`.
- `TradingState` is already `#[derive(Serialize, Deserialize)]` (verified at `crates/scorpio-core/src/state/trading_state.rs:82`), so no custom serialization.

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

    /// Suppress the default terminal report.
    #[arg(long = "no-terminal")]
    pub no_terminal: bool,

    /// Also write a pretty-printed JSON report to --output-dir.
    #[arg(long)]
    pub json: bool,

    /// Directory for file-based reporters. Defaults to the current directory.
    #[arg(long, value_name = "DIR", default_value = ".")]
    pub output_dir: PathBuf,
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
        let started_at = Utc::now();
        let analysis = AnalysisRuntime::new(cfg).await?;
        let state = analysis.run(&args.symbol).await?;
        let ctx = ReportContext {
            symbol: args.symbol.clone(),
            run_id: Uuid::new_v4(),
            started_at,
            finished_at: Utc::now(),
            output_dir: args.output_dir.clone(),
        };
        let failures = chain.run_all(&state, &ctx).await;
        if failures > 0 {
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

`AnalyzeArgs` derives `Clone` so it crosses the `spawn_blocking` boundary, and `Default` for test ergonomics.

## Iteration 1 Non-Goals

Spelled out to prevent scope creep:

- No `--markdown`, `--tg`, `--slack`, `--email`, `--webhook` in iteration 1. Each lands in its own PR with its own feature flag.
- No explicit `--json <path>` / `--markdown <path>` overrides. Filename is auto-derived; users steer via `--output-dir`.
- No parallel reporter execution. Sequential is sufficient; swap later if a network reporter's latency becomes a pain.
- No new config keys in `~/.scorpio-analyst/config.toml`. Terminal and JSON reporters don't need them. Telegram/Slack will add their own in their iterations.
- No dynamic plugin loading (dylib, WASM). All reporters are compile-time.
- No TUI/GUI work. The trait is designed to be reusable by a future TUI, but no code in `scorpio-reporters` depends on a CLI.

## Critical Files

| Purpose | Path |
| --- | --- |
| New crate root | `crates/scorpio-reporters/Cargo.toml` (NEW) |
| Trait + chain + context | `crates/scorpio-reporters/src/lib.rs` (NEW) |
| Terminal reporter (moved) | `crates/scorpio-reporters/src/terminal/` (moved from `crates/scorpio-cli/src/report/`) |
| JSON reporter | `crates/scorpio-reporters/src/json.rs` (NEW) |
| CLI args struct | `crates/scorpio-cli/src/cli/mod.rs` (edited — `Analyze` becomes `Analyze(AnalyzeArgs)`) |
| Chain build + dispatch | `crates/scorpio-cli/src/cli/analyze.rs` (edited — replace `println!` emit with `chain.run_all`) |
| CLI lib module list | `crates/scorpio-cli/src/lib.rs` (edited — drop `pub mod report;`) |
| Main dispatch | `crates/scorpio-cli/src/main.rs` (edited — destructure `AnalyzeArgs`) |
| Workspace members | `Cargo.toml` (edited — add `"crates/scorpio-reporters"`) |
| Workspace deps | `Cargo.toml` (edited — add `scorpio-reporters`; move `colored` / `comfy-table` if needed) |

## Reused Code and Utilities

- `scorpio_core::state::TradingState` at `crates/scorpio-core/src/state/trading_state.rs:82` — already `Serialize + Deserialize`.
- `scorpio_core::data::symbol::validate_symbol` — unchanged, still called from the CLI.
- `scorpio_core::app::AnalysisRuntime` — unchanged entry point.
- `async-trait`, `chrono`, `uuid`, `serde_json`, `tokio` — all existing workspace deps; no new crate additions needed for iteration 1.
- `tracing::error!` — already used project-wide for structured error logs.

## Migration Steps

1. Add `crates/scorpio-reporters/` (`Cargo.toml`, `src/lib.rs` skeleton). Register in root `Cargo.toml` `[workspace] members`.
2. Move `crates/scorpio-cli/src/report/` → `crates/scorpio-reporters/src/terminal/`. Update `mod.rs` to re-export `format_final_report` as `pub(crate)` and define `TerminalReporter`.
3. Shift `colored` and `comfy-table` deps from `scorpio-cli`'s `[dependencies]` to `scorpio-reporters`'.
4. Add `crates/scorpio-reporters/src/json.rs` with `JsonReporter`.
5. In `crates/scorpio-cli/`: add `scorpio-reporters = { workspace = true }` dep; drop `pub mod report;` from `lib.rs`; update `cli/mod.rs` to switch `Commands::Analyze` to `Analyze(AnalyzeArgs)`; update `cli/analyze.rs` to the new `run(&AnalyzeArgs)` signature and chain dispatch; update `main.rs` destructure.
6. Update existing tests in `cli/analyze.rs` that call `run("AAPL")` → `run(&AnalyzeArgs { symbol: "AAPL".into(), ..Default::default() })`.
7. Add targeted tests (below).

## Testing Strategy

New tests inside `crates/scorpio-reporters/`:

- `tests/chain.rs` — fail-soft contract: a reporter that returns `Err` doesn't prevent later reporters from running; `run_all` returns the correct failure count.
- `tests/json.rs` — round-trip: `JsonReporter` writes a valid JSON file; reading and `serde_json::from_str::<TradingState>` yields an equivalent struct. Use `tempfile::tempdir()` for `output_dir`.
- `tests/terminal.rs` — smoke: `TerminalReporter::emit` on a fixture state returns `Ok(())`. (Don't snapshot ANSI output — colored terminal output is brittle; the port is behavior-preserving by construction.)

Existing tests in `crates/scorpio-cli/src/cli/analyze.rs` (the three
symbol-validation tests plus the config-missing tests) update to pass
`AnalyzeArgs` instead of `&str`. They still exercise the same guards.

Existing tests in `crates/scorpio-cli/tests/` (release-archive contract) are
untouched.

## Verification

End-to-end checks (to run after implementation):

1. `cargo fmt -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` — match CI gate.
2. `cargo nextest run --workspace --all-features --locked --no-fail-fast` — all existing + new tests green.
3. `cargo run -- analyze AAPL` — identical terminal output to pre-refactor.
4. `cargo run -- analyze AAPL --json` — terminal output **plus** `./AAPL-<timestamp>.json` written; file round-trips through `serde_json::from_str::<TradingState>`.
5. `cargo run -- analyze AAPL --no-terminal --json --output-dir /tmp/reports` — stdout silent except for the JSON confirmation line; JSON lands under `/tmp/reports/`.
6. `cargo run -- analyze AAPL --json --output-dir /nonexistent/dir` — terminal report prints (leg 1 succeeds), JSON leg logs a `tracing::error!`, process exits non-zero ("1 reporter(s) failed"). Confirms fail-soft.
7. `cargo run -- analyze --help` shows the new flags with correct help text.
