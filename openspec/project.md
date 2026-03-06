# Project Context

## Purpose
Scorpio-analyst is a Rust-native reimplementation of the [TradingAgents](https://github.com/TauricResearch/TradingAgents/) framework (originally Python/LangGraph), based on the paper [arXiv:2412.20138](https://arxiv.org/pdf/2412.20138). It simulates a trading firm with specialized AI agent roles that collaborate through a 5-phase execution pipeline to make autonomous, explainable financial trading decisions.

The system aims to replace monolithic trading AI with a structured multi-agent society — analysts, researchers, traders, risk managers, and a fund manager — achieving superior risk-adjusted returns (26.62% cumulative return, 0.91% max drawdown on backtests) while preserving full decision auditability.

## Tech Stack
- **Language**: Rust 1.93+ (edition 2024)
- **LLM Orchestration**: `rig-core` 0.31 — unified LLM provider abstraction (OpenAI, Anthropic, Gemini, custom Copilot via ACP)
- **Workflow Engine**: `graph-flow` 0.4 (feature `"rig"`) — stateful directed graph orchestration (LangGraph equivalent)
- **Async Runtime**: `tokio`
- **Financial Data**: `finnhub` 0.2 (fundamentals/news), `yfinance-rs` 0.7 (OHLCV pricing)
- **Technical Analysis**: `kand` 0.2 — pure Rust f64 indicators (RSI, MACD, ATR, etc.)
- **Serialization**: `serde` / `serde_json`
- **Error Handling**: `thiserror` (typed domain errors), `anyhow` (context propagation)
- **Rate Limiting**: `governor` (shared via `Arc` across concurrent agents)
- **Observability**: `tracing` / `tracing-subscriber`
- **Config**: `dotenvy` (.env loading), `config.toml` for defaults
- **Security**: `secrecy` (API key management — zeroed on drop, excluded from Debug/logs)
- **Testing**: `mockall` (mocks), `proptest` (property-based)
- **CLI**: `clap` (subcommand-based CLI framework)
- **Interactive TUI (Phase 2)**: `ratatui` + `crossterm` — full-screen interactive terminal UI inspired by Claude Code
- **Desktop UI (Phase 3)**: `gpui` — GPU-accelerated native UI framework from Zed (behind `gui` feature flag)
- **Build**: Cargo (single binary crate, name: `scorpio-analyst`)

## Project Conventions

### Code Style
- Follow the [Rust Style Guide](https://doc.rust-lang.org/book/) and use `rustfmt` for formatting
- Follow [RFC 430](https://github.com/rust-lang/rfcs/blob/master/text/0430-finalizing-naming-conventions.md) naming conventions
- Lines under 100 characters when possible
- Prefer borrowing (`&T`) over cloning; use `&str` over `String` for function params when ownership isn't needed
- Use iterators over index-based loops
- Avoid `unwrap()`/`expect()` — prefer `?` operator and `Result<T, E>`
- No unnecessary `unsafe` code
- Implement common traits (`Debug`, `Clone`, `PartialEq`) on all public types
- Use enums over flags/booleans for type safety
- Use builders for complex object creation
- Keep `main.rs` minimal — move logic to modules (`lib.rs` + named module files)
- Detailed conventions in `.github/instructions/rust.instructions.md`

### Architecture Patterns
- **5-phase pipeline**: Analyst Team (fan-out) → Researcher Team (cyclic debate) → Trader (sequential) → Risk Team (fan-out + debate) → Fund Manager (sequential)
- **State management**: All inter-agent data flows through a strongly-typed `TradingState` struct via `graph_flow::Context` — agents read/write specific struct fields, not free-text chat buffers (eliminates "telephone effect")
- **Dual-tier LLM routing**: Analysts use quick-thinking models (gpt-4o-mini, claude-haiku, gemini-flash); Researchers, Trader, and Risk agents use deep-thinking models (o3, claude-opus, etc.)
- **Concurrency**: Fan-out tasks use `tokio::spawn` with per-field `Arc<RwLock<Option<T>>>` locking (not struct-level). Never hold `std::sync::Mutex` across `.await` — use `tokio::sync::RwLock`
- **Custom GitHub Copilot provider**: Implemented as a custom `rig` provider via ACP (Agent Client Protocol) over JSON-RPC 2.0/NDJSON, spawning `copilot --acp --stdio`
- **Error resilience**: Retry with exponential backoff (max 3, base 500ms); 1 analyst failure degrades gracefully, 2+ aborts; per-analyst 30s timeout
- **Configuration layering**: `config.toml` → `.env` (dotenvy) → environment variables (highest priority)
- **User interaction (phased)**:
  - **Phase 1 (MVP)**: CLI via `clap` — structured subcommands (`analyze`, `backtest`, `config`, `history`) plus natural language queries via `ask` subcommand (LLM intent parser routes to pipeline actions); supports human-readable, JSON, and quiet output modes; real-time agent progress streaming via `tracing`
  - **Phase 2**: Interactive terminal UI via `ratatui`/`crossterm` — persistent conversational session (like Claude Code) building on Phase 1's natural language support with multi-turn context, live agent activity panels, inline trade proposal review/approval, and keyboard-navigable history browsing
  - **Phase 3**: Native desktop application via [GPUI](https://www.gpui.rs/) (Zed's GPU-accelerated Rust UI framework) — live workflow dashboard, trade proposal review, historical audit trail, performance analytics charts; built behind `--features gui` Cargo flag sharing the same core `lib.rs`

### Testing Strategy
- **Unit tests**: Each agent task tested in isolation with mocked API responses (`mockall`). Assertions verify correct `TradingState` fields populated with properly deserialized structs
- **Integration tests**: Full `graph-flow` workflow end-to-end with deterministic stubs (no real API calls). Validates phase transitions, debate cycle termination, and risk moderation loop
- **Backtesting**: Ingest historical OHLCV data (June–November 2024), replay day-by-day with no look-ahead bias. Compute Cumulative Return, Annualized Return, Sharpe Ratio, and Maximum Drawdown
- **Property-based tests**: `proptest` validates `TradingState` serialization round-trips and `TradingError` edge cases
- **CI**: Warnings treated as errors (`-D warnings`). Run `cargo fmt -- --check`, `cargo clippy`, `cargo test`

### Git Workflow
- Spec-driven development via OpenSpec — create change proposals before implementing features, breaking changes, or architecture shifts
- Bug fixes, typos, and non-breaking dependency updates can be done directly without proposals
- Archive completed changes after deployment (`openspec archive <change-id>`)

## Domain Context
- **Trading Agents paradigm**: A multi-agent LLM system that models a real trading firm's organizational structure with specialized roles (analysts, researchers, traders, risk managers, fund manager)
- **Telephone effect**: Data degradation that occurs when agents communicate via unstructured natural language — mitigated by strongly-typed state structs with `serde_json` serialization
- **Dialectical debate**: Bullish and Bearish researchers argue in rounds to prevent confirmation bias, moderated by a Debate Moderator with configurable `max_debate_rounds`
- **Risk personas**: Aggressive (wider stops for momentum), Conservative (capital preservation, veto on overbought/high-beta), Neutral (Sharpe Ratio optimization)
- **Key metrics**: Cumulative Return, Annualized Return, Sharpe Ratio, Maximum Drawdown
- **Technical indicators**: RSI (overbought >70, oversold <30), MACD (signal line crossovers), ATR (volatility measurement)

## Important Constraints
- The project is in **early development** — `Cargo.toml` has no dependencies yet, only `src/main.rs` exists
- API keys must be wrapped in `secrecy::SecretString` and never appear in `Debug` output or logs
- Rate limiting is mandatory — concurrent agents must not exceed provider limits (e.g., Finnhub 30 req/s)
- LLMs must return data in rigid JSON schemas (enforced via `serde`) — no free-text state passing
- Fan-out failures: 1 analyst failure = continue with partial data; 2+ = abort cycle
- Per-analyst timeout: 30 seconds default via `tokio::time::timeout`
- No `std::sync::Mutex` across `.await` boundaries — `tokio::sync::RwLock` only

## External Dependencies
- **LLM Providers**: OpenAI (gpt-4o-mini, o3), Anthropic (claude-haiku, claude-opus), Google Gemini (gemini-flash, advanced reasoning), GitHub Copilot (via ACP/JSON-RPC 2.0)
- **Financial Data APIs**: Finnhub (fundamentals, earnings, news — 30 req/s free tier), Yahoo Finance via `yfinance-rs` (OHLCV pricing), Alpha Vantage (fallback for currencies/crypto)
- **Social Data**: Reddit (r/wallstreetbets, r/investing), X/Twitter APIs — for Sentiment Analyst
- **UI Framework (Phase 2)**: [GPUI](https://www.gpui.rs/) — GPU-accelerated native Rust UI framework from the creators of [Zed](https://zed.dev)
- **Reference Implementation**: [TauricResearch/TradingAgents](https://github.com/TauricResearch/TradingAgents/) (Python/LangGraph)
- **Reference Paper**: [arXiv:2412.20138](https://arxiv.org/pdf/2412.20138)
