<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

## Project Overview

Scorpio-Analyst is a Rust-native reimplementation of the [TradingAgents](https://github.com/TauricResearch/TradingAgents/) framework, a multi-agent LLM-powered financial trading system simulating a trading firm with specialized agent roles, based on the paper [arXiv:2412.20138](https://arxiv.org/pdf/2412.20138).

The system follows a 5-phase execution pipeline orchestrated by `graph-flow`, with `rig-core` agents:

1. Analyst Team (parallel fan-out): Fundamental, Sentiment, News, Technical analysts
2. Researcher Team (cyclic debate): Bullish vs. Bearish researchers moderated
3. Trader Agent: Synthesizes into TradeProposal
4. Risk Management Team (parallel + cyclic): Aggressive, Conservative, Neutral risk agents
5. Fund Manager: Final approve/reject

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

- **State management**: All inter-agent data flows through strongly-typed `TradingState` struct via `graph_flow::Context` — agents read/write specific struct fields, not free-text chat buffers.
- **Dual-tier LLM routing**: Analysts use quick-thinking models (gpt-4o-mini, claude-haiku, gemini-flash); Researchers, Trader, and Risk agents use deep-thinking models (o3, claude-opus, etc.).
- **Concurrency**: Fan-out tasks use `tokio::spawn`. Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState` (not a single struct-level lock). Never hold `std::sync::Mutex` across `.await` — use `tokio::sync::RwLock`.
- **Custom GitHub Copilot provider**: Implemented as a custom `rig` provider via ACP (Agent Client Protocol) over JSON-RPC 2.0/NDJSON, spawning `copilot --acp --stdio`.

## Codebase Structure

- `src/config.rs` - Configuration loading from `config.toml`, `.env`, env vars
- `src/error.rs` - `TradingError` enum with `thiserror`
- `src/lib.rs` - Library root, exports modules
- `src/main.rs` - CLI entry point
- `src/observability.rs` - Tracing setup
- `src/rate_limit.rs` - Global rate limiting with `governor`
- `src/agents/` - Agent implementations: `analyst/`, `researcher/`, `risk/`
- `src/backtest/` - Backtesting framework
- `src/cli/` - CLI commands with `clap`
- `src/data/` - Financial data providers: `finnhub.rs`, `yfinance.rs`
- `src/indicators/` - Technical indicators (planned: RSI, MACD, ATR with `kand`)
- `src/providers/` - LLM providers: `copilot.rs` (custom ACP), `factory.rs`
- `src/state/` - Typed state structs: `trading_state.rs` (main), `fundamental.rs`, `news.rs`, `sentiment.rs`, `technical.rs`, `proposal.rs`, `risk.rs`, `execution.rs`, `token_usage.rs`
- `src/workflow/` - Graph-flow orchestration

## Dependencies

Current key crates:

- `rig-core` 0.32 - LLM provider abstraction
- `serde` / `serde_json` - Serialization
- `thiserror` / `anyhow` - Error handling
- `tokio` - Async runtime
- `tracing` / `tracing-subscriber` - Observability
- `governor` - Rate limiting
- `config` / `dotenvy` - Configuration
- `secrecy` - API key management
- `finnhub` / `yfinance-rs` - Financial data

Planned: `graph-flow`, `kand`, `mockall`, `proptest`.

## Configuration Loading Order

1. `config.toml` — non-sensitive defaults (checked in)
2. `.env` via `dotenvy` — local secrets (git-ignored)
3. Environment variables — CI/CD overrides

## Error Handling Pattern

- `thiserror` for typed `TradingError` enum (variants: `AnalystError`, `RateLimitExceeded`, `NetworkTimeout`, `SchemaViolation`, `Rig`)
- `anyhow` for flexible context propagation within tasks
- Retry: exponential backoff (max 3 retries, base 500ms) for LLM calls
- Graceful degradation: 1 analyst failure continues with partial data; 2+ failures abort the cycle
- Per-analyst timeout: 30s default via `tokio::time::timeout`

## Rust Guidelines

Detailed Rust coding conventions are in `.github/instructions/rust.instructions.md`. Key points:
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed.
- Use `serde` for serialization, `thiserror`/`anyhow` for errors.
- Async code uses `tokio` runtime with `async/await`.
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on public types.
- Use enums over flags/booleans for type safety.
- Warnings are treated as errors in CI (`-D warnings`).

<!-- OPENSPEC:END -->