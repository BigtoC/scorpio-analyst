# `financial-data` Capability

## ADDED Requirements

### Requirement: Finnhub Fundamental Data Client

The system MUST provide an async Finnhub client wrapper that retrieves corporate fundamentals, earnings reports, and
insider transaction data for a given asset symbol. The client MUST accept the shared Finnhub rate limiter via its
constructor and MUST await limiter readiness before each outbound API request. Return types MUST map to the
`FundamentalData` sub-struct defined in `core-types`, including revenue growth, Price-to-Earnings ratio, liquidity
ratios, and insider transactions. All Finnhub transport and deserialization errors MUST be mapped to the established
`TradingError` hierarchy (`NetworkTimeout`, `RateLimitExceeded`, `SchemaViolation`).

#### Scenario: Successful Fundamental Data Retrieval

- **WHEN** the Finnhub client is called with a valid asset symbol and the Finnhub API responds successfully
- **THEN** the client returns a populated `FundamentalData` struct containing revenue growth, P/E ratio, liquidity
  ratio, and insider transaction data

#### Scenario: Rate Limiter Throttles Concurrent Requests

- **WHEN** multiple concurrent analyst tasks invoke the Finnhub client simultaneously and the aggregate request rate
  approaches the provider limit
- **THEN** the client awaits the shared rate limiter before dispatching each request, preventing 429 errors from the
  upstream API

#### Scenario: Finnhub API Timeout

- **WHEN** a Finnhub API request exceeds the network timeout boundary
- **THEN** the client returns a `TradingError::NetworkTimeout` with provider context identifying Finnhub as the source

### Requirement: Finnhub News Data Client

The system MUST provide an async Finnhub client function that retrieves market news and economic indicator data for a
given asset symbol. The return type MUST map to the `NewsData` sub-struct defined in `core-types`, including articles,
macro events, and causal relationships relevant to the target asset. The news client MUST share the same rate limiter
and error mapping conventions as the fundamental data client.

#### Scenario: Successful News Retrieval

- **WHEN** the news client is called with a valid asset symbol and the Finnhub API responds successfully
- **THEN** the client returns a populated `NewsData` struct containing recent news articles and macro event data
  relevant to the queried symbol

#### Scenario: Finnhub News API Returns Empty Results

- **WHEN** the Finnhub news endpoint returns an empty result set for the queried symbol
- **THEN** the client returns a valid `NewsData` struct with empty collections rather than an error, allowing the
  downstream News Analyst to report the absence of recent news

### Requirement: Yahoo Finance OHLCV Client

The system MUST provide an async Yahoo Finance client wrapper that retrieves historical OHLCV (Open, High, Low, Close,
Volume) pricing data for a given asset symbol and date range. The client MUST use the `yfinance-rs` crate's async
builder pattern. The return type MUST be a `Vec<Candle>` (or equivalent typed OHLCV struct) suitable for consumption
by the Technical Analyst and by the downstream `add-technical-analysis` change for `kand` indicator calculations.
All transport and deserialization errors MUST be mapped to `TradingError`.

#### Scenario: Successful OHLCV Retrieval

- **WHEN** the Yahoo Finance client is called with a valid symbol and date range
- **THEN** the client returns a chronologically ordered `Vec<Candle>` containing OHLCV data points for each trading
  day in the requested range

#### Scenario: Invalid Date Range

- **WHEN** the Yahoo Finance client is called with an end date preceding the start date
- **THEN** the client returns a `TradingError` indicating invalid input rather than dispatching a malformed request

#### Scenario: Yahoo Finance Throttling

- **WHEN** Yahoo Finance throttles or temporarily blocks the request
- **THEN** the client maps the failure to `TradingError::NetworkTimeout` with retry context, and the foundation's
  exponential backoff retry policy applies

### Requirement: Rig Tool Wrappers For Financial Data Functions

The system MUST expose data retrieval functions as `rig` `#[tool]`-annotated wrappers so downstream analyst agents
can bind them as typed tools through the agent builder helper from `llm-providers`. Each tool wrapper MUST be a thin
async function that delegates to the underlying client method and returns a result compatible with `rig`'s tool
response schema. This capability includes tool wrappers for Finnhub fundamentals, earnings, insider transactions,
news, and Yahoo Finance OHLCV retrieval. Tool wrappers MUST be defined alongside their respective client
implementations within `src/data/`.

#### Scenario: Analyst Agent Binds Finnhub Tool

- **WHEN** a downstream Fundamental Analyst agent is constructed using the agent builder helper
- **THEN** it can attach the Finnhub fundamentals tool via `rig`'s typed tool interface, and the tool returns
  `FundamentalData` when invoked by the LLM during agent execution

#### Scenario: Analyst Agent Binds OHLCV Tool

- **WHEN** a downstream Technical Analyst agent is constructed
- **THEN** it can attach the Yahoo Finance OHLCV tool, and the tool returns `Vec<Candle>` data when invoked

### Requirement: Financial Data Module Boundary

This capability's implementation MUST remain limited to structured financial market data concerns within
`src/data/mod.rs`, `src/data/finnhub.rs`, and `src/data/yfinance.rs`. It MUST re-export all public types and
functions needed by downstream fundamental, news, and technical agent changes from `src/data/mod.rs`. Social-media
sentiment scraping, embedding, and vector retrieval MUST be handled by the separate `add-sentiment-data` capability.
The financial-data module MUST NOT modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`,
`src/rate_limit.rs`) or provider-owned files (`src/providers/*`).

#### Scenario: Downstream Agent Import Path

- **WHEN** a downstream agent change imports data functions
- **THEN** it uses `use scorpio_analyst::data::{...}` and receives the Finnhub client, Yahoo Finance client,
  and financial-data tool wrappers through a single module path

#### Scenario: No Foundation File Modifications

- **WHEN** the financial data module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`, and
  `src/rate_limit.rs` remain unmodified, as all dependencies and module declarations were pre-declared by
  `add-project-foundation`
