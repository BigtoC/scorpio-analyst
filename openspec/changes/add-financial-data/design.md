# Design for `add-financial-data`

## Context

The `add-project-foundation` change established core types (`FundamentalData`, `TechnicalData`, `SentimentData`,
`NewsData`), error handling (`TradingError`), configuration (`ApiConfig` with provider credentials), and rate limiting
(`governor`-based shared limiters). The `add-llm-providers` change established `rig-core` integration and tool-binding
patterns. This change fills in the structured financial market data ingestion layer (`src/data/`) that downstream
analyst agents depend on to populate `TradingState` fields. Social sentiment ingestion is intentionally deferred to the
dedicated `add-sentiment-data` change so this design remains limited to Finnhub and Yahoo Finance responsibilities.

**Stakeholders:** Analyst Team agents (Fundamental, News, Technical), rate-limiting infrastructure, configuration (API
keys and endpoints), `add-technical-analysis` (consumes OHLCV data from yfinance).

## Goals / Non-Goals

- **Goals:**
  - Wrap the `finnhub` crate to expose typed async functions for fundamentals, earnings, news, and insider
    transactions, returning data that maps to `FundamentalData` and `NewsData` sub-structs.
  - Wrap the `yfinance-rs` crate to expose typed async OHLCV retrieval for a given symbol and date range,
    returning `Vec<Candle>` (or equivalent) consumed by the Technical Analyst and `kand` calculations.
  - Accept the shared `governor` rate limiter via constructor injection for all clients that hit rate-limited APIs.
  - Expose `rig` `#[tool]`-compatible wrappers so downstream analyst agents can bind data functions as typed tools.
  - Map all data-layer errors into the established `TradingError` hierarchy.
  - Confine all implementation to `src/data/` without modifying foundation-owned files.

- **Non-Goals:**
  - Implementing the Technical Analyst's `kand` indicator calculations — that belongs to `add-technical-analysis`.
  - Implementing agent logic, system prompts, or LLM invocations — those belong to `add-analyst-team`.
  - Implementing sentiment scraping, embedding, or vector retrieval — those belong to `add-sentiment-data`.
  - Real-time streaming price feeds — the MVP operates on historical/snapshot data.
  - Persistent market-data caching across runs — caching can be introduced later if API pressure justifies it.

## Architectural Overview

```
src/data/
├── mod.rs       ← Re-exports financial data public API
├── finnhub.rs   ← Finnhub crate wrapper (fundamentals, earnings, news, insider txns)
├── yfinance.rs  ← yfinance-rs wrapper (OHLCV historical pricing)
```

### Data Flow

```
  Finnhub API ──► finnhub.rs ──► FundamentalData, NewsData ──► TradingState
  Yahoo Finance ──► yfinance.rs ──► Vec<Candle> ──► (kand) ──► TechnicalData
```

### Client Pattern

Each data client follows a consistent pattern:

1. **Constructor** accepts configuration references (`&ApiConfig`) and a shared rate limiter
   (`Arc<DefaultDirectRateLimiter>`).
2. **Async methods** await rate limiter readiness before each outbound request.
3. **Return types** map directly to `core-types` sub-structs or intermediate types that downstream code transforms.
4. **Errors** map to `TradingError` variants (`NetworkTimeout`, `RateLimitExceeded`, `SchemaViolation`).

### Tool Wrappers

Each data function is exposed as a `rig` `#[tool]`-annotated wrapper so downstream analyst agents can bind them
through the agent builder helper. The tool wrappers are thin async functions that delegate to the underlying client
methods. Tool definitions live alongside their client implementations in the respective source files.

## Key Decisions

- **Decision: Use `finnhub` crate directly rather than raw HTTP** — The `finnhub` crate provides 96% API coverage
  with strongly typed Rust models, automatic rate limiting, and retry logic. Wrapping it adds a thin layer for
  `TradingError` mapping and rate limiter injection rather than reimplementing HTTP client logic.
  - *Alternatives considered:* Raw `reqwest` calls would provide more control but duplicate the typed model
    definitions and error handling already provided by the crate.

- **Decision: Use `yfinance-rs` async builder pattern** — The crate's fluent builder supports parallel fetching
  and async execution natively, aligning with the `tokio` runtime requirements.
  - *Alternatives considered:* Raw API fetching, which would be unnecessary given the available rust ecosystem support.

- **Decision: Yahoo Finance is the only MVP pricing source** — The current scope keeps OHLCV ingestion centered on
  `yfinance-rs` rather than adding a secondary fallback provider. This keeps the pricing path simpler while still
  satisfying the Technical Analyst's historical-data needs.
  - *Alternatives considered:* A second pricing provider could be added later if concrete Yahoo coverage gaps appear,
    but it is not required for the current financial-data scope.

- **Decision: Rate limiter injection via constructor** — Aligns with the `rate-limiting` capability's dependency
  injection requirement. Each client receives the appropriate provider-scoped limiter rather than constructing its
  own.
  - *Alternatives considered:* Global static limiter would be simpler but violates the per-provider scoping
    requirement and makes testing harder.

- **Decision: Tool wrappers co-located with client code** — `#[tool]` functions are defined in the same file as
  their underlying client to keep the data layer self-contained. Downstream agent changes import these tools without
  reaching into client internals.
  - *Alternatives considered:* A separate `src/data/tools.rs` file was considered but adds indirection without
    clear benefit given the small number of tools per client.

## Risks / Trade-offs

- **Finnhub free-tier limits** — The free tier allows 30 req/s. The shared rate limiter enforces this, but concurrent
  analyst agents making multiple Finnhub calls per run could approach the budget. Mitigation: the rate limiter queues
  excess requests rather than failing; analyst timeout (30s) provides a ceiling.

- **yfinance-rs data availability** — Yahoo Finance occasionally throttles or blocks automated access.
  Mitigation: retry logic via `TradingError::NetworkTimeout` and the foundation's exponential backoff apply to
  yfinance-rs calls.

## Open Questions

- Should the Finnhub client cache responses for the duration of a single analysis run to avoid redundant API calls
  when multiple agents query overlapping data? Recommendation: defer caching to post-MVP; the rate limiter and
  30s timeout provide sufficient protection for now.
- Should the Yahoo Finance wrapper normalize timestamps into trading-day date-only values at the data boundary, or
  preserve provider timestamps until `add-technical-analysis` consumes them? Recommendation: preserve provider
  timestamps in `Candle` and let downstream indicator code normalize only if needed.
