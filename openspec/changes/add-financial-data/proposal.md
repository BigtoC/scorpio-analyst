# Change: Add Financial Market Data Ingestion Layer

## Why

The Analyst Team's Fundamental, News, Technical, and current Sentiment paths require reliable, rate-limited access to
structured market data APIs before any agent can populate `TradingState` fields. This proposal introduces the core
financial data ingestion layer that wraps Finnhub (fundamentals, earnings, news, insider transactions) and yfinance-rs
(OHLCV pricing) behind typed async clients. Company-specific news retrieved through this layer supports both the News
Analyst and the MVP Sentiment Analyst baseline, while direct social-platform ingestion is intentionally deferred to
future improvements. All clients accept the shared `governor`-based rate limiter via dependency injection, ensuring
concurrent analyst agents cannot exceed provider quotas.

## What Changes

- Implement a Finnhub client wrapper (`src/data/finnhub.rs`) exposing `get_fundamentals()`, `get_earnings()`,
  `get_news()`, and `get_insider_transactions()`. Return types map to `FundamentalData` and `NewsData` sub-structs
  from `core-types`. The client accepts the shared Finnhub rate limiter and awaits readiness before each outbound
  request.
- Implement a Yahoo Finance client wrapper (`src/data/yfinance.rs`) exposing `get_ohlcv(symbol, start, end)` that
  returns a `Vec<Candle>` (or equivalent typed OHLCV struct). This data feeds the Technical Analyst and is passed to
  `kand` for indicator calculation by the downstream `add-technical-analysis` change.
- Wire the financial data module's public API through `src/data/mod.rs`, re-exporting the Finnhub and Yahoo Finance
  client types and functions needed by downstream agent changes.
- Define `rig` `#[tool]` wrappers for each Finnhub and Yahoo Finance data function so downstream analyst agents can
  bind them as typed tools through the agent builder helper from `add-llm-providers`.

## Impact

- Affected specs: `financial-data` (new)
- Affected code: `src/data/mod.rs` (fill in skeleton), `src/data/finnhub.rs` (new), `src/data/yfinance.rs` (new)
- Dependencies: `add-project-foundation` (core types, error handling, config, rate-limiting, module stubs),
  `add-llm-providers` (rig embeddings/vector store integration, tool macro patterns)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`,
  `src/providers/factory.rs`)
