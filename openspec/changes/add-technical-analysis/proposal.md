# Change: Add Technical Indicator Calculation Layer

## Why

The Technical Analyst agent requires pre-calculated indicator values (RSI, MACD, ATR, Bollinger Bands, moving averages,
VWMA, and others) before it can reason about price action. LLMs cannot perform precise iterative math on large
time-series arrays, so the system must compute these indicators natively in Rust and inject the results into the LLM
context. This proposal introduces the `kand`-based technical analysis calculation layer that transforms raw OHLCV
candle data (provided by `add-financial-data`) into a populated `TechnicalData` struct ready for agent consumption.
The MVP is designed around traditional OHLCV-based long-term investing workflows and is not fully compatible with
crypto-specific analysis needs such as logarithmic-scale interpretation, on-chain valuation metrics like MVRV, and
24/7 market-structure assumptions.

## What Changes

- Implement a technical indicator calculator module (`src/indicators/calculator.rs`) that accepts `Vec<Candle>` from
  the Yahoo Finance client and computes technical indicators using the `kand` crate in `f64` extended precision. The
  calculator MUST cover the Technical Analyst prompt's exact indicator names (`close_50_sma`, `close_200_sma`,
  `close_10_ema`, `macd`, `macds`, `macdh`, `rsi`, `boll`, `boll_ub`, `boll_lb`, `atr`, `vwma`) and support the
  architect plan's broader batch calculation target of 60+ indicators from raw OHLCV input.
- Expose individual indicator functions (`calculate_rsi()`, `calculate_macd()`, `calculate_atr()`,
  `calculate_bollinger_bands()`, `calculate_sma()`, `calculate_ema()`, `calculate_vwma()`), a prompt-compatible named
  indicator selection API, and a batch `calculate_all_indicators()` function that populates the `TechnicalData`
  sub-struct from `core-types`, including support/resistance boundaries derived from price data.
- Wire the indicators module's public API through `src/indicators/mod.rs`, re-exporting calculator types and
  functions needed by the downstream Technical Analyst agent.
- Define `rig` `#[tool]`-annotated wrappers for the indicator calculation functions so the Technical Analyst agent
  can bind them as typed tools through the agent builder helper from `llm-providers`. These wrappers MUST operate on
  pre-fetched candle data from `financial-data` rather than fetching market data directly, preserving the planned
  boundary between data retrieval and indicator calculation.
- Explicitly defer full crypto-native analysis to future improvements rather than treating the MVP OHLCV indicator
  layer as fully sufficient for digital-asset workflows.

## Impact

- Affected specs: `technical-analysis` (new)
- Affected code: `src/indicators/mod.rs` (fill in skeleton), `src/indicators/calculator.rs` (new)
- Dependencies: `add-project-foundation` (core types including `TechnicalData`, error handling, module stubs),
  `add-llm-providers` (rig tool macro patterns), `add-financial-data` (provides `Vec<Candle>` input type)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`, `src/rate_limit.rs`)
  or data-layer-owned files (`src/data/*`)
