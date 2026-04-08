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
cargo run             # Run the binary
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

```
src/
├── main.rs                    # CLI entrypoint (config → tracing → pipeline → report)
├── lib.rs                     # Public module exports
├── config.rs                  # Configuration loading (TOML + env)
├── error.rs                   # TradingError enum + RetryPolicy
├── constants.rs               # Constants
├── observability.rs           # Tracing/logging setup
├── rate_limit.rs              # Governor-based rate limiting
│
├── agents/                    # LLM agent implementations
│   ├── analyst/               # Phase 1: fundamental, sentiment, news, technical
│   ├── researcher/            # Phase 2: bullish, bearish, moderator
│   ├── trader/                # Phase 3: trade proposal synthesis
│   ├── risk/                  # Phase 4: aggressive, neutral, conservative, moderator
│   ├── fund_manager/          # Phase 5: final approve/reject
│   └── shared/                # json.rs (schema enforcement), prompt.rs, usage.rs
│
├── state/                     # Shared pipeline state
│   ├── trading_state.rs       # TradingState (all inter-agent data)
│   ├── fundamental.rs         # FundamentalData
│   ├── technical.rs           # TechnicalData
│   ├── sentiment.rs           # SentimentData
│   ├── news.rs                # NewsData
│   ├── proposal.rs            # TradeProposal
│   ├── risk.rs                # RiskReport
│   ├── execution.rs           # ExecutionStatus (Approved/Rejected)
│   └── token_usage.rs         # TokenUsageTracker
│
├── workflow/                  # Graph orchestration (graph-flow)
│   ├── pipeline.rs            # TradingPipeline (5-phase DAG runner)
│   ├── context_bridge.rs      # Bridge between graph-flow::Context & TradingState
│   ├── snapshot.rs            # Phase snapshots to SQLite (SnapshotStore)
│   └── tasks/                 # Per-phase task implementations
│       ├── analyst.rs         # Phase 1: fan-out analysts
│       ├── research.rs        # Phase 2: researcher debate loop
│       ├── trading.rs         # Phase 3: trader synthesis
│       ├── risk.rs            # Phase 4: risk debate loop
│       └── accounting.rs      # Token usage reporting
│
├── data/                      # Market data clients
│   ├── finnhub.rs             # Finnhub API (fundamentals, earnings, news, insiders)
│   ├── fred.rs                # FRED API (macro indicators: CPI, inflation)
│   ├── yfinance.rs            # Yahoo Finance (OHLCV bars)
│   └── symbol.rs              # Symbol resolution
│
├── indicators/                # Technical indicator calculation (kand-based)
│   ├── core_math.rs           # RSI, MACD, ATR, Bollinger, SMA, EMA, VWMA
│   ├── batch.rs               # calculate_all_indicators
│   ├── support_resistance.rs  # Support/resistance level derivation
│   ├── tools.rs               # rig tool wrappers (#[tool] structs)
│   └── types.rs               # MacdResult, BollingerResult, etc.
│
├── providers/                 # LLM provider factory (rig-core)
│   ├── mod.rs                 # ModelTier (QuickThinking/DeepThinking), ProviderId enum
│   ├── factory/               # create_completion_model, build_agent, prompt_with_retry
│   ├── copilot.rs             # GitHub Copilot via ACP
│   └── acp.rs                 # Agent Client Protocol (JSON-RPC 2.0/NDJSON)
│
├── report/                    # Final report formatting
├── cli/                       # CLI module
└── backtest/                  # Backtesting framework (skeleton)
```

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
- **Phased UI**: Phase 1 = CLI (`clap`); Phase 2 = interactive TUI (`ratatui`/`crossterm`); Phase 3 = native desktop
  app (`gpui`, behind `--features gui`). All phases share the same core `lib.rs`.

### Crate Dependencies

| Crate                              | Purpose                                                                            |
|------------------------------------|------------------------------------------------------------------------------------|
| `rig-core` 0.32                    | LLM provider abstraction (OpenAI, Anthropic, Gemini, custom Copilot)               |
| `graph-flow` 0.5 (feature `"rig"`) | Stateful directed graph orchestration (LangGraph equivalent)                       |
| `schemars` 1                       | JSON schema generation for `#[tool]` macros                                        |
| `finnhub` 0.2                      | Corporate fundamentals, earnings, news, insider transactions                       |
| `yfinance-rs` 0.7                  | Historical OHLCV pricing data                                                      |
| `kand` 0.2                         | Technical indicators (RSI, MACD, ATR, Bollinger, SMA, EMA, VWMA) in pure Rust f64  |
| `tokio` 1 (full)                   | Async runtime                                                                      |
| `serde` / `serde_json`             | State serialization                                                                |
| `thiserror` 2 / `anyhow` 1         | Error handling (thiserror for typed domain errors, anyhow for context propagation) |
| `governor` 0.8                     | Global rate limiting (shared via `Arc` across concurrent agents)                   |
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

**Dev dependencies:** `proptest` 1, `mockall` 0.13, `pretty_assertions` 1, `tempfile` 3, `paft-money` 0.7, `rust_decimal` 1.

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

1. `config.toml` — non-sensitive defaults (checked in)
2. `.env` via `dotenvy` — local secrets (git-ignored)
3. Environment variables — CI/CD overrides (prefix: `SCORPIO__`, e.g. `SCORPIO__LLM__MAX_DEBATE_ROUNDS=5`)

### Error Handling Pattern

- `thiserror` for the `TradingError` enum (typed variants: `AnalystError`, `RateLimitExceeded`, `NetworkTimeout`,
  `SchemaViolation`, `Rig`, `Config`, `Storage`, `GraphFlow`)
- `anyhow` for flexible context propagation within tasks
- Retry: exponential backoff (max 3 retries, base 500ms) for LLM calls via `RetryPolicy`
- Graceful degradation: 1 analyst failure continues with partial data; 2+ failures abort the cycle
- Per-analyst timeout: configurable via `analyst_timeout_secs` (default 3000s) via `tokio::time::timeout`

### Running & Debugging

```bash
RUST_LOG=debug cargo run                              # Full trace output
SCORPIO__TRADING__ASSET_SYMBOL=AAPL cargo run          # Override ticker
SCORPIO__LLM__MAX_DEBATE_ROUNDS=1 cargo run            # Quick test (1 debate round)
```

### Common Development Tasks

- **Adding a new agent**: Implement agent in `src/agents/<role>/`, add phase task in `src/workflow/tasks/`
- **Adding a new data source**: Implement client in `src/data/`, expose as rig tool via `#[tool]` macro
- **Adding an indicator**: Add math function to `src/indicators/core_math.rs`, expose via tool struct in `tools.rs`
- **Adding a provider**: Extend `ProviderId` enum in `src/providers/mod.rs`, add case in `factory::create_completion_model()`

## CI/CD

GitHub Actions (`.github/workflows/tests.yml`):
- Triggers on push/PR to `main` (only when `src/`, `tests/`, `Cargo.toml`, or `Cargo.lock` change)
- Installs Protobuf compiler (required by dependencies)
- Steps: `cargo fmt -- --check` → `cargo clippy --all-targets -- -D warnings` → `cargo nextest run --all-features --locked`

## Rust Guidelines

Detailed Rust coding conventions are in `.github/instructions/rust.instructions.md`. Key points:
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed.
- Use `serde` for serialization, `thiserror`/`anyhow` for errors.
- Async code uses `tokio` runtime with `async/await`.
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on public types.
- Use enums over flags/booleans for type safety.
- Warnings are treated as errors in CI (`-D warnings`).
