# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust-native reimplementation of the [TradingAgents](https://github.com/TauricResearch/TradingAgents/) framework (
originally Python/LangGraph). This is a multi-agent LLM-powered financial trading system that simulates a trading firm
with specialized agent roles. Based on the paper [arXiv:2412.20138](https://arxiv.org/pdf/2412.20138).

The project is in early development — see PRD.md for the full specification.

## Build Commands

```bash
cargo build           # Build the project
cargo run -p scorpio-cli -- --help   # Run the CLI binary
cargo test            # Run all tests
cargo test <name>     # Run a single test by name
cargo clippy          # Lint
cargo fmt             # Format code
cargo fmt -- --check  # Check formatting without modifying
```

Requires **Rust 1.93+** (edition 2024).

## Architecture

The system follows a 5-phase execution pipeline orchestrated by `graph-flow`, with `rig-core` agents as the cognitive
layer:

1. **Analyst Team** (parallel fan-out) — Fundamental, Sentiment, News, Technical analysts fetch and interpret market
   data concurrently
2. **Researcher Team** (cyclic debate) — Bullish vs. Bearish researchers argue in rounds, moderated by a Debate
   Moderator (`max_debate_rounds`)
3. **Trader Agent** (sequential) — Synthesizes debate into a structured `TradeProposal`
4. **Risk Management Team** (parallel fan-out + cyclic debate) — Aggressive, Conservative, Neutral risk agents debate,
   coordinated by a Risk Moderator (`max_risk_rounds`)
5. **Fund Manager** (sequential) — Final approve/reject decision, with deterministic fallback: reject if Conservative + Neutral risk agents both flag a violation

### Source Layout

The repository is a Cargo workspace with two active crates under `crates/`:

```
crates/
├── scorpio-core/              # Shared runtime/domain crate (library, publish = false)
│   ├── Cargo.toml
│   ├── migrations/            # sqlx::migrate! resolves via scorpio-core's CARGO_MANIFEST_DIR
│   │   ├── 0001_create_phase_snapshots.sql
│   │   └── 0002_add_symbol_and_schema_version.sql
│   └── src/
│       ├── lib.rs             # pub mod declarations + `pub use app::AnalysisRuntime`
│       ├── app/               # Application facade (AnalysisRuntime::new / ::run)
│       ├── settings.rs        # PartialConfig + atomic load/save (non-interactive)
│       ├── config.rs          # Runtime Config loader (env > user file > defaults)
│       ├── constants.rs       # Constants (HEALTH_CHECK_TIMEOUT_SECS, etc.)
│       ├── error.rs           # TradingError + RetryPolicy
│       ├── observability.rs   # Tracing/logging setup (used by every surface)
│       ├── rate_limit.rs      # Governor-based rate limiting
│       ├── agents/            # LLM agent implementations (analyst/researcher/trader/risk/fund_manager/shared)
│       ├── state/             # Shared pipeline state (TradingState + per-phase types)
│       ├── workflow/          # Graph orchestration (TradingPipeline, tasks, snapshot/**)
│       ├── data/              # Market data clients (finnhub/fred/yfinance/symbol/adapters)
│       ├── indicators/        # Technical indicators (kand-based)
│       ├── providers/         # LLM provider factory (rig-core, copilot ACP)
│       ├── analysis_packs/    # Pack manifests + runtime policy
│       └── backtest/          # Backtesting skeleton (core-internal per R13)
│
└── scorpio-cli/               # Binary crate hosting the user-facing CLI
    ├── Cargo.toml             # Depends on scorpio-core
    └── src/
        ├── main.rs            # #[tokio::main] entry; dispatch analyze/setup/upgrade
        ├── lib.rs             # pub mod cli; pub mod report; (library surface for in-crate tests)
        ├── cli/
        │   ├── mod.rs         # Cli + Commands structs; clap derive
        │   ├── analyze.rs     # Thin wrapper: load config → validate → AnalysisRuntime → print report
        │   ├── update.rs      # Release check + `scorpio upgrade` self-update
        │   └── setup/
        │       ├── mod.rs     # Wizard orchestrator, recovery UX, run()
        │       └── steps.rs   # Interactive step fns (1-5) + pure helpers
        └── report/            # Final terminal report formatting (CLI-only per R23)
```

Core integration tests live in `crates/scorpio-core/tests/` (pipeline, state, app facade, observability, foundation); CLI integration tests live in `crates/scorpio-cli/tests/` (release-archive contract only).

### Key Design Decisions

- **State management**: All inter-agent data flows through a strongly-typed `TradingState` struct via
  `graph_flow::Context` — agents read/write specific struct fields, not free-text chat buffers. This eliminates the "
  telephone effect" where data degrades through natural language handoffs.
- **Dual-tier LLM routing**: Analysts use quick-thinking models (gpt-4o-mini, claude-haiku, gemini-flash); Researchers,
  Trader, and Risk agents use deep-thinking models (o3, claude-opus, etc.). Configured via `ModelTier` enum +
  `ProviderId` enum (OpenAI, Anthropic, Gemini, Copilot, OpenRouter).
- **Concurrency**: Fan-out tasks use `tokio::spawn`. Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState` (not a
  single struct-level lock). Never hold `std::sync::Mutex` across `.await` — use `tokio::sync::RwLock`.
- **Custom GitHub Copilot provider**: Implemented as a custom `rig` provider via ACP (Agent Client Protocol) over
  JSON-RPC 2.0/NDJSON, spawning `copilot --acp --stdio`.
- **Token usage tracking**: Every LLM call records model ID, wall-clock latency, and provider-reported token counts
  into a `TokenUsageTracker` on `TradingState`. Providers that don't expose authoritative counts (e.g. Copilot via ACP)
  record documented unavailable metadata. Per-phase and per-agent breakdowns are displayed after every run.
- **Phase snapshots**: Each pipeline phase persists its output to SQLite (`SnapshotStore`) for audit trail and recovery.
- **TradingState schema evolution**: `TradingState` is serialized into `phase_snapshots.trading_state_json`. Old snapshots
  may not deserialize with a newer struct. Rules:
  - Every new field on `TradingState` **must** carry `#[serde(default)]`; omitting it makes all existing snapshots
    unreadable.
  - When a field is **renamed**, **removed**, or has its **type changed** in a backward-incompatible way, bump
    `THESIS_MEMORY_SCHEMA_VERSION` in `src/workflow/snapshot/thesis.rs`. The thesis lookup skips rows whose version
    does not match the constant *in either direction* (newer or older), so bumping it explicitly retires incompatible
    data and a binary downgrade after the bump still ignores newer rows safely.
  - The thesis lookup degrades gracefully (warn + skip) when deserialization fails. The `warn!` line emits only
    `symbol`, `schema_version`, and `error.kind = "deserialize"` — never `serde_json` error text, which can echo
    payload bytes. Relying on warn-and-skip for every deploy is still a smell; `#[serde(default)]` + version bumps
    are the real fix.
  - Snapshotted state structs serialized into `phase_snapshots.trading_state_json` (anything reachable from `TradingState` via serde) must not use `#[serde(deny_unknown_fields)]` — it converts every additive field into a backward-incompatible change. This rule does NOT apply to RPC, tool-argument, or config types where typo detection is more valuable than forward-compat.
- **Pack-owned prompts (centralized)**: `AnalysisPackManifest.prompt_bundle` is the single source of every system
  prompt for active packs. The runtime contract:
  - `PreflightTask` is the sole writer of `state.analysis_runtime_policy`, the sole runner of
    `validate_active_pack_completeness`, and the sole writer of `KEY_RUNTIME_POLICY` / `KEY_ROUTING_FLAGS` to context.
  - Active packs must populate every required prompt slot for the configured topology (analysts, debate stage when
    `max_debate_rounds > 0`, risk stage when `max_risk_rounds > 0`, plus trader and fund manager). Failures surface
    as `TaskExecutionFailed` from preflight before any analyst or model task fires.
  - Prompt builders take `&RuntimePolicy` directly; the renderer reads `policy.prompt_bundle.<role>` with no legacy
    fallback. The exhaustive `Role` → `PromptSlot` match in `workflow/topology.rs` makes adding a `Role` variant a
    compile error until the role-to-slot table is extended.
  - `{analysis_emphasis}` substitution is sanitized at preflight (strict 0x20–0x7E ASCII, role-injection-tag
    rejection, ≤256 chars). `{ticker}` is not re-validated by this refactor — it continues to flow through the
    existing `validate_symbol` syntactic gate plus data-API existence chain.
- **Topology-driven routing**: `RoutingFlags` (written to `KEY_ROUTING_FLAGS` by preflight) governs *entry* into the
  debate and risk stages. Loop-back conditionals (`round < max`) keep using the per-iteration round counters. Tests
  that bypass preflight should hydrate runtime policy via `crate::testing::with_baseline_runtime_policy`.
- **Phased UI**: Phase 1 = CLI (`clap` + `inquire`) — **done**; `scorpio analyze <SYMBOL>` runs the pipeline, `scorpio setup` is an interactive wizard that writes `~/.scorpio-analyst/config.toml`. Phase 2 = interactive TUI (`ratatui`/`crossterm`); Phase 3 = native desktop app (`gpui`, behind `--features gui`). All phases depend on `scorpio-core` — the shared crate exposes `AnalysisRuntime`, `settings::PartialConfig`, and the runtime `Config` type as the preferred entry points.

### Crate Dependencies

| Crate                              | Purpose                                                                            |
|------------------------------------|------------------------------------------------------------------------------------|
| `rig-core` 0.32                    | LLM provider abstraction (OpenAI, Anthropic, Gemini, custom Copilot)               |
| `graph-flow` 0.5 (feature `"rig"`) | Stateful directed graph orchestration (LangGraph equivalent)                       |
| `schemars` 1                       | JSON schema generation for `#[tool]` macros                                        |
| `clap` 4 (feature `"derive"`)      | CLI argument parsing (`scorpio analyze <SYMBOL>`, `scorpio setup`)                 |
| `inquire` 0.9                      | Interactive setup wizard prompts (Password, Select, Confirm)                       |
| `toml` 1                           | Serialise `PartialConfig` to `~/.scorpio-analyst/config.toml`                      |
| `tempfile` 3                       | Atomic config writes (`NamedTempFile` + rename)                                    |
| `finnhub` 0.2                      | Corporate fundamentals, earnings, news, insider transactions                       |
| `yfinance-rs` 0.7                  | Historical OHLCV pricing data                                                      |
| `kand` 0.2                         | Technical indicators (RSI, MACD, ATR, Bollinger, SMA, EMA, VWMA) in pure Rust f64  |
| `tokio` 1 (full)                   | Async runtime                                                                      |
| `serde` / `serde_json`             | State serialization                                                                |
| `thiserror` 2 / `anyhow` 1         | Error handling (thiserror for typed domain errors, anyhow for context propagation) |
| `governor` 0.10                    | Global rate limiting (shared via `Arc` across concurrent agents)                   |
| `tracing` / `tracing-subscriber`   | Structured observability (json + env-filter features)                              |
| `secrecy` 0.10                     | API key management (zeroed on drop, excluded from Debug/logs)                      |
| `config` 0.15 / `dotenvy` 0.15     | TOML config loading + .env file support                                            |
| `reqwest` 0.13                     | HTTP client (json + query features)                                                |
| `sqlx` 0.8                         | SQLite for phase snapshot persistence                                              |
| `uuid` 1                           | Unique execution IDs (v4 + serde)                                                  |
| `chrono` 0.4                       | Date/time handling                                                                 |
| `async-trait` 0.1                  | Async trait support                                                                |
| `colored` 3 / `comfy-table` 7      | Human-readable output formatting                                                   |
| `figlet-rs` 1.0                    | ASCII art header                                                                   |
| `futures` 0.3                      | Async combinators                                                                  |
| `nonzero_ext` 0.3                  | Non-zero integer utilities                                                         |

**Dev dependencies:** `proptest` 1, `mockall` 0.13, `pretty_assertions` 1, `paft-money` 0.7, `rust_decimal` 1, `tempfile` 3, `flate2` 1, `tar` 0.4, `zip` 8.

Shared dep versions are pinned centrally under `[workspace.dependencies]` in the root `Cargo.toml`; each crate consumes them via `foo.workspace = true`. Core owns the runtime dep set (rig-core, graph-flow, kand, finnhub, yfinance-rs, sqlx, secrecy, config, dotenvy, governor, schemars, nonzero_ext). CLI owns the presentation/binary-specific set (clap, inquire, colored, comfy-table, figlet-rs, self_update, semver, sha2, hex). Dual-consumed deps (tokio, serde, serde_json, anyhow, thiserror, tracing, chrono, uuid, reqwest, async-trait, futures, tempfile) live as workspace entries.

## Work Mode
> Based on the complexity of the tasks, choose the appropriate work mode

### Direct Execution Model (Default)

Trigger: bug fixes, small features, <30 line changes
Behavior: write code directly, do not invoke any skills

### Full Development Mode

Trigger: user explicitly says "full flow" or uses one of the `/full` command.
Behavior: follow this sequence strictly:
1. `/superpowers:brainstorming` — requirements exploration
2. `/ce:plan` — technical plan, auto-search `docs/solutions/`
3. `/superpowers:test-driven-development` — TDD implementation
4. `/ce:review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
5. `/ce:compound` — knowledge consolidation

### Coding Mode

Trigger: User explicitly says "write code" or uses `/opsx:apply` or `/spec-code-developer`.
1. `/superpowers:test-driven-development` — TDD implementation
2. `/ce:review` — multi-agent code review, code quality checks should also reference `.github/instructions/rust.instructions.md`.
3. `/ce:compound` — knowledge consolidation

## Knowledge Consolidation

After resolving a non-trivial problem, run `/ce:compound` to persist the solution for future reference.

- `docs/solutions/` — documented solved problems (bug fixes, best practices, workflow patterns), organized by category
- `/ce:plan` auto-searches `docs/solutions/` at planning time to surface relevant prior solutions before implementation begins
- Each solution document includes: problem description, root cause, fix applied, and tags for search

When to invoke `/ce:compound`:
- After a tricky bug is fixed (especially build/CI failures, async issues, borrow-checker patterns)
- After establishing a new architectural pattern or workflow convention
- After integrating a new dependency or provider that required non-obvious configuration

### Configuration Loading Order

**Precedence (highest wins):** env vars > user file > compiled defaults.

1. `~/.scorpio-analyst/config.toml` — written by `scorpio setup`; flat `PartialConfig` (API keys + routing). Created with `0o600` permissions.
2. `.env` via `dotenvy` — local env overrides (git-ignored), loaded before the config crate pipeline.
3. `SCORPIO__*` environment variables — CI/CD overrides (double-underscore separator, e.g. `SCORPIO__LLM__MAX_DEBATE_ROUNDS=5`). Wins over the user file on any overlapping field.
4. `SCORPIO_*_API_KEY` env vars — secret injection; always override the corresponding key from the user file (with a `tracing::warn!` on collision).

The project-level `config.toml` at the repo root is **not read at runtime** — it is inert and kept only to avoid disrupting existing workspaces. See the deprecation notice inside the file itself.

### Error Handling Pattern

- `thiserror` for the `TradingError` enum (typed variants: `AnalystError`, `RateLimitExceeded`, `NetworkTimeout`,
  `SchemaViolation`, `Rig`, `Config`, `Storage`, `GraphFlow`)
- `anyhow` for flexible context propagation within tasks
- Retry: exponential backoff (max 3 retries, base 500ms) for LLM calls via `RetryPolicy`
- Graceful degradation: 1 analyst failure continues with partial data; 2+ failures abort the cycle
- Per-analyst timeout: configurable via `analyst_timeout_secs` (default 3000s) via `tokio::time::timeout`

### Running & Debugging

```bash
cargo run -p scorpio-cli -- setup                     # Interactive wizard → ~/.scorpio-analyst/config.toml
cargo run -p scorpio-cli -- analyze AAPL              # Run pipeline for AAPL
cargo run -p scorpio-cli -- analyze --help            # Show analyze flags
RUST_LOG=debug cargo run -p scorpio-cli -- analyze AAPL   # Full trace output
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run -p scorpio-cli -- analyze AAPL   # Quick test (1 debate round)
cargo run -p scorpio-cli -- --version                 # Print version
```

Use `cargo run -p scorpio-cli -- …` from the repo root to target the CLI crate explicitly after the workspace split.

### Common Development Tasks

| Task                  | Files to touch                                                                                                                                                                                                 |
|-----------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| New agent             | `crates/scorpio-core/src/agents/<role>/`, `crates/scorpio-core/src/workflow/tasks/`                                                                                                                            |
| New data source       | `crates/scorpio-core/src/data/`, expose via `#[tool]` macro                                                                                                                                                    |
| New indicator         | `crates/scorpio-core/src/indicators/core_math.rs` + `crates/scorpio-core/src/indicators/tools.rs`                                                                                                              |
| New LLM provider      | Extend `ProviderId` in `crates/scorpio-core/src/providers/mod.rs`, add case in `crates/scorpio-core/src/providers/factory/`                                                                                    |
| New analysis pack     | Add `PackId` variant in `crates/scorpio-core/src/analysis_packs/manifest/pack_id.rs`, add match arm in `crates/scorpio-core/src/analysis_packs/builtin.rs`                                                     |
| New CLI subcommand    | Add variant to `Commands` in `crates/scorpio-cli/src/cli/mod.rs`, create `crates/scorpio-cli/src/cli/<name>.rs`, dispatch in `crates/scorpio-cli/src/main.rs`                                                  |
| New wizard config key | Add field to `PartialConfig` in `crates/scorpio-core/src/settings.rs`, add step in `crates/scorpio-cli/src/cli/setup/steps.rs`, inject in `Config::load_from_user_path` in `crates/scorpio-core/src/config.rs` |

## CI/CD

GitHub Actions (`.github/workflows/tests.yml`):
- Triggers on push/PR to `main` (only when `crates/**`, `.cargo/**`, `Cargo.toml`, or `Cargo.lock` change, plus the workflow file itself)
- Installs Protobuf compiler (required by dependencies)
- Steps: `cargo fmt -- --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## Rust Guidelines

Detailed Rust coding conventions are in `.github/instructions/rust.instructions.md`. Key points:
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed.
- Use `serde` for serialization, `thiserror`/`anyhow` for errors.
- Async code uses `tokio` runtime with `async/await`.
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on public types.
- Use enums over flags/booleans for type safety.
- Warnings are treated as errors in CI (`-D warnings`).
