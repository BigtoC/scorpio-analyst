# Tasks for `add-technical-analysis`

## Prerequisites

- [x] `add-project-foundation` is complete (core types including `TechnicalData`, error handling, module stubs)
- [x] `add-llm-providers` is complete (rig-core integration and tool macro patterns)
- [x] `add-financial-data` is complete (provides `Vec<Candle>` input type from `yfinance-rs`)

## 1. Individual Indicator Functions (`src/indicators/refactored_calc/core_math.rs`)

- [x] 1.1 Define `IndicatorCalculator` struct (or stateless module functions) that accepts `&[Candle]` as input
- [x] 1.2 Implement `calculate_rsi(candles: &[Candle], period: usize)` -- compute RSI via `kand` in `f64`,
      return `Result<Vec<Option<f64>>, TradingError>`; return `SchemaViolation` for empty input
- [x] 1.3 Implement `calculate_macd(candles: &[Candle], fast: usize, slow: usize, signal: usize)` -- compute
      MACD line, signal line, and histogram via `kand`; return a typed MACD result struct
- [x] 1.4 Implement `calculate_atr(candles: &[Candle], period: usize)` -- compute Average True Range via `kand`;
      return `Result<Vec<Option<f64>>, TradingError>`
- [x] 1.5 Implement `calculate_bollinger_bands(candles: &[Candle], period: usize, std_dev: f64)` -- compute
      middle, upper, and lower bands; return a typed Bollinger result struct
- [x] 1.6 Implement `calculate_sma(candles: &[Candle], period: usize)` -- compute Simple Moving Average via
      `kand`; return `Result<Vec<Option<f64>>, TradingError>`
- [x] 1.7 Implement `calculate_ema(candles: &[Candle], period: usize)` -- compute Exponential Moving Average via
      `kand`; return `Result<Vec<Option<f64>>, TradingError>`
- [x] 1.8 Implement `calculate_vwma(candles: &[Candle], period: usize)` -- compute Volume-Weighted Moving Average
      via `kand`; return `Result<Vec<Option<f64>>, TradingError>`
- [x] 1.9 Implement deterministic support/resistance boundary derivation from OHLCV price action for inclusion in
      `TechnicalData`
- [x] 1.10 Add a prompt-compatible named indicator selection API that accepts the exact indicator names used by the
      Technical Analyst prompt (`close_50_sma`, `close_200_sma`, `close_10_ema`, `macd`, `macds`, `macdh`, `rsi`,
      `boll`, `boll_ub`, `boll_lb`, `atr`, `vwma`) and routes them to the correct calculator functions
- [x] 1.11 Handle edge cases: empty candle arrays, insufficient candle count for lookback period, and NaN
      propagation -- represent unavailable values as `None` rather than NaN

## 2. Batch Indicator Calculation

- [x] 2.1 Implement `calculate_all_indicators(candles: &[Candle])` -- invoke all individual indicator functions
      with default periods (RSI 14, MACD 12/26/9, ATR 14, Bollinger 20/2.0, SMA 50/200, EMA 10, VWMA 20),
      assemble results into the `TechnicalData` struct from `core-types`, including support/resistance levels
- [x] 2.2 Ensure the batch function returns partial results when some indicators cannot be computed due to
      insufficient candle count (e.g., 200 SMA requires 200+ candles) -- populate computable indicators and
      leave others as `None`
- [x] 2.3 Map all `kand` computation errors to `TradingError::SchemaViolation` with descriptive context
- [x] 2.4 Structure the internal calculator to scale toward the architect plan's 60+ indicator batch target without
      changing the public module boundary required by this change

## 3. Rig Tool Wrappers

- [x] 3.1 Define `rig` `#[tool]`-annotated wrapper function for `calculate_all_indicators` that accepts
      pre-fetched candle data, computes all indicators, and returns `TechnicalData`
- [x] 3.2 Define `rig` `#[tool]`-annotated wrapper functions for individual indicator calculations
      (`calculate_rsi`, `calculate_macd`, `calculate_atr`, `calculate_bollinger_bands`) that accept candle
      data and return typed results
- [x] 3.3 Define a `rig` `#[tool]`-annotated named indicator selection wrapper that accepts the exact prompt-facing
      indicator names and candle data, returning only the requested indicator payloads
- [x] 3.4 Ensure tool wrappers return results compatible with `rig`'s tool response schema and preserve the
      architectural boundary that `financial-data` owns candle retrieval

## 4. Indicators Module Wiring (`src/indicators/mod.rs`)

- [x] 4.1 Fill in the `src/indicators/mod.rs` skeleton with calculator module wiring
- [x] 4.2 Re-export public types (intermediate result structs for MACD, Bollinger, etc.), calculator functions,
      and tool wrappers from the module root
- [x] 4.3 Verify downstream import path `use scorpio_analyst::indicators::{...}` resolves all re-exported types

## 5. Unit Tests

- [x] 5.1 Write unit tests for `calculate_rsi` with known OHLCV data verifying RSI values against expected
      results (overbought > 70, oversold < 30 boundary cases)
- [x] 5.2 Write unit tests for `calculate_macd` verifying MACD line, signal line, and histogram values
- [x] 5.3 Write unit tests for `calculate_atr` verifying volatility measurement against known data
- [x] 5.4 Write unit tests for `calculate_bollinger_bands` verifying upper/lower bands relative to middle band
- [x] 5.5 Write unit tests for `calculate_sma` and `calculate_ema` verifying moving average values
- [x] 5.6 Write unit tests for `calculate_vwma` verifying volume-weighted calculations
- [x] 5.7 Write unit tests for support/resistance derivation verifying deterministic boundaries from known price
      action inputs
- [x] 5.8 Write unit tests for the named indicator selection API verifying each prompt-facing indicator name maps
      to the correct calculator output
- [x] 5.9 Write unit tests for `calculate_all_indicators` verifying full `TechnicalData` population
- [x] 5.10 Write unit tests for edge cases: empty candle array returns `TradingError::SchemaViolation`,
      insufficient candles for 200 SMA returns partial results with `None` for that indicator
- [x] 5.11 Write unit tests verifying `f64` precision -- no unexpected NaN propagation across indicator chains

## 6. Integration Tests

- [x] 6.1 Write integration test: construct `Vec<Candle>` from mock OHLCV data (200+ candles), invoke
      `calculate_all_indicators`, verify complete `TechnicalData` struct population including support/resistance
- [x] 6.2 Write integration test: pipeline from `YFinanceClient` mock -> `calculate_all_indicators` ->
      `TechnicalData`, verifying the full data flow from candle retrieval to indicator output

## 7. Documentation and CI

- [x] 7.1 Add inline doc comments (`///`) for all public types and functions in `calculator.rs` and `mod.rs`
- [x] 7.2 Document that the MVP indicator layer is designed for traditional OHLCV-based long-term investing and is
      not fully compatible with crypto-native analysis requirements (log-scale interpretation, MVRV, 24/7 market
      assumptions), which remain future improvements
- [x] 7.3 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [x] 7.4 Ensure `cargo fmt -- --check` passes
- [x] 7.5 Ensure `cargo test` passes all new and existing tests
