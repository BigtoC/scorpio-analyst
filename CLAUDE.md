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

### Key Design Decisions

- **State management**: All inter-agent data flows through a strongly-typed `TradingState` struct via
  `graph_flow::Context` — agents read/write specific struct fields, not free-text chat buffers. This eliminates the "
  telephone effect" where data degrades through natural language handoffs.
- **Dual-tier LLM routing**: Analysts use quick-thinking models (gpt-4o-mini, claude-haiku, gemini-flash); Researchers,
  Trader, and Risk agents use deep-thinking models (o3, claude-opus, etc.).
- **Concurrency**: Fan-out tasks use `tokio::spawn`. Per-field `Arc<RwLock<Option<T>>>` locking on `TradingState` (not a
  single struct-level lock). Never hold `std::sync::Mutex` across `.await` — use `tokio::sync::RwLock`.
- **Custom GitHub Copilot provider**: Implemented as a custom `rig` provider via ACP (Agent Client Protocol) over
  JSON-RPC 2.0/NDJSON, spawning `copilot --acp --stdio`.
- **Token usage tracking**: Every LLM call records model ID, wall-clock latency, and provider-reported token counts
  into a `TokenUsageTracker` on `TradingState`. Providers that don't expose authoritative counts (e.g. Copilot via ACP)
  record documented unavailable metadata. Per-phase and per-agent breakdowns are displayed after every run.
- **Phased UI**: Phase 1 = CLI (`clap`); Phase 2 = interactive TUI (`ratatui`/`crossterm`); Phase 3 = native desktop
  app (`gpui`, behind `--features gui`). All phases share the same core `lib.rs`.

### Planned Crate Dependencies

| Crate                              | Purpose                                                                            |
|------------------------------------|------------------------------------------------------------------------------------|
| `rig-core` 0.31                    | LLM provider abstraction (OpenAI, Anthropic, Gemini, custom Copilot)               |
| `graph-flow` 0.4 (feature `"rig"`) | Stateful directed graph orchestration (LangGraph equivalent)                       |
| `finnhub` 0.2                      | Corporate fundamentals, earnings, news                                             |
| `yfinance-rs` 0.7                  | Historical OHLCV pricing data                                                      |
| `kand` 0.2                         | Technical indicators (RSI, MACD, ATR) in pure Rust f64                             |
| `tokio`                            | Async runtime                                                                      |
| `serde` / `serde_json`             | State serialization                                                                |
| `anyhow` / `thiserror`             | Error handling (anyhow for context propagation, thiserror for typed domain errors) |
| `governor`                         | Global rate limiting (shared via `Arc` across concurrent agents)                   |
| `tracing` / `tracing-subscriber`   | Structured observability                                                           |
| `secrecy`                          | API key management (zeroed on drop, excluded from Debug/logs)                      |
| `dotenvy`                          | .env file loading                                                                  |
| `clap`                             | Subcommand-based CLI framework                                                     |
| `colored` / `comfy-table`          | Human-readable output formatting                                                   |
| `ratatui` + `crossterm`            | Interactive TUI (Phase 2, behind feature flag)                                     |
| `gpui`                             | GPU-accelerated native desktop UI (Phase 3, behind `gui` feature flag)             |
| `mockall` / `proptest`             | Testing (mocks + property-based)                                                   |

### Configuration Loading Order

1. `config.toml` — non-sensitive defaults (checked in)
2. `.env` via `dotenvy` — local secrets (git-ignored)
3. Environment variables — CI/CD overrides

### Error Handling Pattern

- `thiserror` for the `TradingError` enum (typed variants: `AnalystError`, `RateLimitExceeded`, `NetworkTimeout`,
  `SchemaViolation`, `Rig`)
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
