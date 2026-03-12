# `technical-analysis` Capability

## ADDED Requirements

### Requirement: Individual Technical Indicator Calculations

The system MUST provide individual indicator calculation functions that accept `&[Candle]` (OHLCV data from the
`financial-data` capability) and compute technical indicators using the `kand` crate in `f64` extended precision. The
following indicators MUST be supported at minimum, with prompt-compatible names available for downstream tool calls:

- **RSI** (Relative Strength Index, prompt name `rsi`) with configurable period (default 14)
- **MACD** (Moving Average Convergence Divergence, prompt names `macd`, `macds`, `macdh`) with configurable
  fast/slow/signal periods (default 12/26/9),
  returning the MACD line, signal line, and histogram as a typed result
- **ATR** (Average True Range, prompt name `atr`) with configurable period (default 14)
- **Bollinger Bands** (prompt names `boll`, `boll_ub`, `boll_lb`) with configurable period and standard deviation
  multiplier (default 20/2.0), returning
  middle, upper, and lower bands as a typed result
- **SMA** (Simple Moving Average, prompt names `close_50_sma` and `close_200_sma`) with configurable period
  (default 50 and 200)
- **EMA** (Exponential Moving Average, prompt name `close_10_ema`) with configurable period (default 10)
- **VWMA** (Volume-Weighted Moving Average, prompt name `vwma`) with configurable period (default 20)

Each function MUST return `Result<T, TradingError>` where unavailable values due to insufficient lookback data are
represented as `Option<f64>` (`None`) rather than NaN. Empty or zero-length input arrays MUST produce a
`TradingError::SchemaViolation` rather than a panic. The implementation SHOULD remain extensible enough to support the
architect plan's broader 60+ indicator batch target without changing the public module boundary introduced by this
capability. This MVP capability is designed for traditional OHLCV-based long-term investing workflows and MUST NOT be
described as fully compatible with crypto-native analysis requirements such as logarithmic-scale interpretation,
on-chain valuation metrics like MVRV, or explicit 24/7 market-structure handling.

#### Scenario: RSI Calculation With Sufficient Data

- **WHEN** the RSI function is called with 200+ candles and a 14-period lookback
- **THEN** it returns a `Vec<Option<f64>>` where the first 13 entries are `None` (insufficient lookback) and
  subsequent entries contain valid RSI values in the range [0, 100]

#### Scenario: MACD Calculation Returns Typed Result

- **WHEN** the MACD function is called with sufficient candle data and default periods (12/26/9)
- **THEN** it returns a typed result containing the MACD line, signal line, and histogram as separate `Vec<Option<f64>>`
  arrays

#### Scenario: Empty Candle Input Rejected

- **WHEN** any individual indicator function is called with an empty candle array
- **THEN** it returns `TradingError::SchemaViolation` with a descriptive error message indicating insufficient data

#### Scenario: Insufficient Lookback Period Handled Gracefully

- **WHEN** an indicator function is called with fewer candles than its lookback period requires
- **THEN** it returns a result where all values are `None` rather than producing NaN or panicking

#### Scenario: Crypto Analysis Expectations Are Scoped Out Of MVP

- **WHEN** a caller attempts to use this MVP capability as a complete crypto-analysis solution
- **THEN** the documentation and proposal scope make clear that OHLCV indicator calculations are reusable but full
  crypto-native analysis remains a future improvement outside the MVP boundary

### Requirement: Batch Indicator Calculation

The system MUST provide a batch function `calculate_all_indicators(candles: &[Candle])` that computes the full suite
of supported technical indicators with default periods (RSI 14, MACD 12/26/9, ATR 14, Bollinger 20/2.0, SMA 50/200,
EMA 10, VWMA 20) and assembles the results into the `TechnicalData` sub-struct defined in `core-types`. The batch
function MUST also derive support/resistance boundaries from the same OHLCV series so the output aligns with the
foundation capability's `TechnicalData` description. The batch function MUST return partial results when some
indicators cannot be computed due to insufficient candle count -- it MUST populate computable indicators and leave
others as `None` in the `TechnicalData` struct. All `kand` computation errors MUST be mapped to
`TradingError::SchemaViolation` with descriptive context.

#### Scenario: Full Batch Calculation With Sufficient Data

- **WHEN** `calculate_all_indicators` is called with 200+ candles of OHLCV data
- **THEN** it returns a fully populated `TechnicalData` struct with RSI, MACD (line, signal, histogram), ATR,
  Bollinger Bands (middle, upper, lower), SMA (50, 200), EMA (10), VWMA (20), and support/resistance boundaries all
  containing valid values

#### Scenario: Partial Results With Limited Data

- **WHEN** `calculate_all_indicators` is called with 100 candles (sufficient for RSI 14, MACD, ATR, Bollinger,
  SMA 50, EMA 10, VWMA but insufficient for SMA 200)
- **THEN** it returns a `TechnicalData` struct where the 200-period SMA field is `None` and all other indicators
  and support/resistance boundaries are populated with valid values

#### Scenario: Batch Calculation Error Mapping

- **WHEN** a `kand` computation produces an error during batch calculation
- **THEN** the error is mapped to `TradingError::SchemaViolation` with context identifying which indicator failed
  and why

### Requirement: F64 Precision Guarantee

All technical indicator calculations MUST use `f64` extended precision throughout the computation pipeline. The system
MUST NOT introduce intermediate `f32` conversions or precision-degrading operations. This prevents the subtle
floating-point errors and NaN propagation issues that occur when calculating iterative indicators (RSI, EMA) over long
time horizons.

#### Scenario: Precision Preserved Over Long Series

- **WHEN** an EMA is calculated over 1000+ candles in `f64` precision
- **THEN** the final values do not exhibit accumulated floating-point drift that would alter trading signal
  interpretation compared to a reference `f64` calculation

#### Scenario: No Intermediate F32 Conversion

- **WHEN** the calculator processes candle data through any indicator function
- **THEN** all intermediate values remain in `f64` precision from input through output, with no narrowing
  conversions to `f32`

### Requirement: Rig Tool Wrappers For Indicator Functions

The system MUST expose indicator calculation functions as `rig` `#[tool]`-annotated wrappers so the downstream
Technical Analyst agent can bind them as typed tools through the agent builder helper from `llm-providers`. Tool
wrappers MUST operate on pre-fetched candle data supplied from the `financial-data` capability rather than fetching
market data directly. Tool wrappers MUST include a batch tool for full `TechnicalData` calculation, individual
indicator tools for targeted calculations, and a named-indicator selection interface that accepts the exact prompt
names (`close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`, `macdh`, `rsi`, `boll`, `boll_ub`,
`boll_lb`, `atr`, `vwma`). Tool wrappers MUST be co-located with the calculator implementation within
`src/indicators/`.

#### Scenario: Technical Analyst Binds Batch Indicator Tool

- **WHEN** the downstream Technical Analyst agent is constructed using the agent builder helper
- **THEN** it can attach the batch indicator calculation tool, and the tool returns a populated `TechnicalData`
  struct when invoked by the LLM during agent execution

#### Scenario: Technical Analyst Binds Individual Indicator Tool

- **WHEN** the Technical Analyst agent needs a specific indicator (e.g., RSI only)
- **THEN** it can attach the individual RSI tool, and the tool returns `Vec<Option<f64>>` RSI values when invoked

#### Scenario: Technical Analyst Uses Prompt-Compatible Indicator Names

- **WHEN** the Technical Analyst prompt requests indicators using the exact names defined in `docs/prompts.md`
- **THEN** the named-indicator tool accepts those names unchanged and returns the corresponding calculator outputs

### Requirement: Technical Analysis Module Boundary

This capability's implementation MUST remain limited to technical indicator calculation concerns within
`src/indicators/mod.rs` and `src/indicators/calculator.rs`. It MUST re-export all public types and functions needed
by the downstream Technical Analyst agent change from `src/indicators/mod.rs`. The technical analysis module MUST NOT
modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`, `src/rate_limit.rs`),
data-layer-owned files (`src/data/*`), or provider-owned files (`src/providers/*`).

#### Scenario: Downstream Agent Import Path

- **WHEN** the downstream Technical Analyst agent change imports indicator functions
- **THEN** it uses `use scorpio_analyst::indicators::{...}` and receives the calculator functions, tool wrappers,
  and intermediate result types through a single module path

#### Scenario: No Foundation Or Data File Modifications

- **WHEN** the technical analysis module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`, and
  `src/rate_limit.rs` remain unmodified, and the data-layer-owned `src/data/*` files remain unmodified, as all
  dependencies and module declarations were pre-declared by `add-project-foundation`
