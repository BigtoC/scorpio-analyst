# Tasks for `add-financial-data`

## Prerequisites

- [ ] `add-project-foundation` is complete (core types, error handling, config, rate-limiting, module stubs)
- [ ] `add-llm-providers` is complete (rig-core integration and tool macro patterns)

## 1. Finnhub Client Wrapper (`src/data/finnhub.rs`)

- [ ] 1.1 Define `FinnhubClient` struct accepting `&ApiConfig` (for the Finnhub API key) and
      `Arc<DefaultDirectRateLimiter>` (shared Finnhub rate limiter) via constructor
- [ ] 1.2 Implement `get_fundamentals(symbol: &str)` — fetch corporate financials and company profile via the
      `finnhub` crate, map response fields to `FundamentalData` (revenue growth, P/E, liquidity ratio, insider
      transactions), await rate limiter before each request
- [ ] 1.3 Implement `get_earnings(symbol: &str)` — fetch quarterly earnings data via the `finnhub` crate, map
      to relevant fields within `FundamentalData`
- [ ] 1.4 Implement `get_insider_transactions(symbol: &str)` — fetch insider transaction data via the `finnhub`
      crate, map to insider transaction fields within `FundamentalData`
- [ ] 1.5 Implement `get_news(symbol: &str)` — fetch market news for the symbol via the `finnhub` crate, map
      response to `NewsData` (articles, macro events, causal relationships), await rate limiter before request
- [ ] 1.6 Map all `finnhub` crate errors to `TradingError` variants: transport failures to `NetworkTimeout`,
      rate limit responses to `RateLimitExceeded`, deserialization failures to `SchemaViolation`
- [ ] 1.7 Define `rig` `#[tool]`-annotated wrapper functions for `get_fundamentals`, `get_earnings`,
      `get_insider_transactions`, and `get_news` that delegate to the `FinnhubClient` methods
- [ ] 1.8 Write unit tests with mocked Finnhub API responses (`mockall`) verifying correct `FundamentalData` and
      `NewsData` population
- [ ] 1.9 Write unit tests verifying rate limiter is awaited before each outbound request
- [ ] 1.10 Write unit tests for error mapping (timeout, rate limit, schema violation)

## 2. Yahoo Finance OHLCV Client (`src/data/yfinance.rs`)

- [ ] 2.1 Define `YFinanceClient` struct accepting configuration references via constructor
- [ ] 2.2 Implement `get_ohlcv(symbol: &str, start: &str, end: &str)` — use `yfinance-rs` async builder to
      fetch historical OHLCV data, return `Vec<Candle>` (define `Candle` struct with `date`, `open`, `high`,
      `low`, `close`, `volume` fields if not provided by the crate)
- [ ] 2.3 Validate input date range (end >= start) and return `TradingError` for invalid ranges
- [ ] 2.4 Map `yfinance-rs` transport and parsing errors to `TradingError::NetworkTimeout` and
      `TradingError::SchemaViolation` respectively
- [ ] 2.5 Define `rig` `#[tool]`-annotated wrapper function for `get_ohlcv` that delegates to the client method
- [ ] 2.6 Write unit tests with mocked yfinance responses verifying correct `Vec<Candle>` output and
      chronological ordering
- [ ] 2.7 Write unit tests for invalid date range rejection
- [ ] 2.8 Write unit tests for error mapping (transport failure, deserialization failure)

## 3. Data Module Re-exports (`src/data/mod.rs`)

- [ ] 3.1 Fill in the `src/data/mod.rs` skeleton with the Finnhub and Yahoo Finance module wiring required by the
      financial-data capability
- [ ] 3.2 Re-export public client types (`FinnhubClient`, `YFinanceClient`), tool wrappers, and supporting type
      `Candle` from the module root
- [ ] 3.3 Verify downstream import path `use scorpio_analyst::data::{...}` resolves all re-exported financial-data
      types

## 4. Integration Tests

- [ ] 4.1 Write integration test: construct `FinnhubClient` with mocked rate limiter, invoke `get_fundamentals`
      and `get_news`, verify `FundamentalData` and `NewsData` population and rate limiter calls
- [ ] 4.2 Write integration test: construct `YFinanceClient`, invoke `get_ohlcv` with mocked responses, verify
      `Vec<Candle>` correctness

## 5. Documentation and CI

- [ ] 5.1 Add inline doc comments (`///`) for all public types and functions in `finnhub.rs`, `yfinance.rs`, and
      `mod.rs`
- [ ] 5.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [ ] 5.3 Ensure `cargo fmt -- --check` passes
- [ ] 5.4 Ensure `cargo test` passes all new and existing tests
