# Design for `add-technical-analysis`

## Context

The `add-project-foundation` change established core types including `TechnicalData` (RSI, MACD, ATR,
support/resistance levels), error handling (`TradingError`), and the module skeleton (`src/indicators/`). The
`add-financial-data` change provides the `Vec<Candle>` OHLCV data from Yahoo Finance via `yfinance-rs`. This change
fills in the technical indicator calculation layer that transforms raw candle data into the `TechnicalData` struct
consumed by the Technical Analyst agent. The MVP design target is long-term investing over traditional OHLCV-based
markets. Crypto assets can still reuse some of the same indicators, but the current MVP is not fully compatible with
crypto-native analysis because it does not include logarithmic-scale interpretation, on-chain valuation metrics (for
example MVRV), or explicit 24/7 market-structure handling.

**Stakeholders:** Technical Analyst agent (primary consumer), `add-analyst-team` (binds indicator tools to the agent),
`add-graph-orchestration` (indicator results flow into `TradingState.technical_indicators`).

## Goals / Non-Goals

- **Goals:**
  - Wrap the `kand` crate to compute momentum, trend, and volatility indicators in `f64` precision.
  - Accept `Vec<Candle>` as input (from `add-financial-data`) and output a populated `TechnicalData` struct.
  - Provide both individual indicator functions (for selective calculation), a prompt-compatible named indicator
    selection API, and a batch function (for the full indicator suite).
  - Expose `rig` `#[tool]`-compatible wrappers so the Technical Analyst agent can invoke calculations via LLM
    tool calls.
  - Handle edge cases: insufficient candle data, NaN propagation, empty input arrays.
  - Populate support/resistance boundaries alongside indicator outputs so `TechnicalData` aligns with the
    foundation capability description.
  - Confine all implementation to `src/indicators/` without modifying foundation or data-layer files.

- **Non-Goals:**
  - Implementing the Technical Analyst agent logic, system prompt, or LLM invocations -- belongs to
    `add-analyst-team`.
  - Fetching OHLCV data from Yahoo Finance -- belongs to `add-financial-data`.
  - Real-time streaming indicator updates -- the MVP operates on historical snapshots.
  - Custom user-defined indicator formulas -- all indicators follow `kand`'s built-in implementations.
  - Full crypto-native technical analysis, including log-scale-aware interpretation, MVRV and other on-chain metrics,
    and crypto-specific market-structure adjustments -- deferred to future improvements.

## Architectural Overview

    src/indicators/
    +-- mod.rs                 <-- Facade re-exporting public API
    +-- core_math.rs           <-- Individual indicator functions
    +-- batch.rs               <-- Batch calculation and named indicator API
    +-- tools.rs               <-- rig #[tool] implementations
    +-- support_resistance.rs   <-- Pivot derivation
    +-- types.rs               <-- Result structs
    +-- utils.rs               <-- Crate-private helpers

The module uses the **Facade Pattern**: submodules are private (mod-level), and `mod.rs` re-exports all public
symbols so downstream code sees a single, consistent public API via `use scorpio_analyst::indicators::*`.

### Data Flow

    Vec<Candle> (from yfinance.rs)
        --> core_math.rs (kand computations in f64)
        --> TechnicalData (written to TradingState.technical_indicators)

Tool wrappers in this capability operate on candle data that has already been fetched through the financial-data
layer. The Technical Analyst agent remains responsible for calling the data-retrieval tool first and then invoking
indicator calculation, preserving the separation required by the architect plan.

### Indicator Categories

Following the Technical Analyst prompt specification (docs/prompts.md), the calculator MUST support
at minimum these exact prompt-facing indicator names:

| Category             | Indicators                                                       |
|----------------------|------------------------------------------------------------------|
| Moving Averages      | `close_50_sma`, `close_200_sma`, `close_10_ema`                  |
| MACD Related         | `macd`, `macds`, `macdh`                                         |
| Momentum             | `rsi`                                                            |
| Volatility           | `boll`, `boll_ub`, `boll_lb`, `atr`                              |
| Volume-Based         | `vwma`                                                           |

The batch function computes all of the above from a single `Vec<Candle>` input, derives support/resistance
boundaries from the same price series, and assembles the `TechnicalData` struct. Beyond these prompt-required
indicators, the internal calculator design should remain extensible enough to support the architect plan's
broader 60+ indicator batch-calculation target without changing the public module boundary.

### Precision and Safety

- All calculations use `f64` extended precision via `kand`'s native Rust implementation.
- NaN values from insufficient lookback periods are represented as `Option<f64>` (None) in the output.
- Functions return `Result<T, TradingError>` -- empty or insufficient input arrays produce
  `TradingError::SchemaViolation` rather than panicking.
- No `unwrap()` or `expect()` on calculation results.
- Prompt-facing indicator selection uses canonical string names mapped internally to strongly typed calculator
  functions so LLM tool calls can use the exact prompt contract without leaking stringly typed logic through the
  whole module.

## Key Decisions

- **`kand` as the sole indicator engine**: Pure Rust, `f64` precision, comprehensive indicator coverage,
  inspired by TA-Lib but without C FFI. No alternative Rust crates match its breadth and precision guarantees.
- **Batch + individual function pattern**: The batch function covers the common case (full indicator suite for
  the Technical Analyst), while individual functions support selective calculation if future agents or
  backtesting need only specific indicators.
- **Long-term support/resistance derivation**: Because this system is oriented toward long-term investors, support
  and resistance should be derived from trailing 104 weeks of OHLCV aggregated into weekly candles rather than from
  daily price noise. The MVP method uses 5-bar weekly swing pivots, includes the 40-week SMA and 52-week high/low as
  anchor levels, clusters nearby candidate levels with `max(2% of current close, 1x weekly ATR(14))` as the zone
  width, and selects the highest-scoring support below price and resistance above price based on repeated touches,
  recency, and relative volume.
- **Prompt-name compatibility layer**: The prompt requires exact indicator names. The public tool layer should
  accept those names and map them to internal calculator functions so agent prompts and Rust code stay aligned.
- **`Vec<Candle>` as input boundary**: Reuses the type defined by `add-financial-data` rather than introducing
  a separate OHLCV representation, keeping the data pipeline consistent.
- **Tool wrappers do not fetch data**: To preserve the planned separation of concerns, this capability owns only
  indicator calculation. Data retrieval stays in `financial-data`, and tool wrappers here operate on pre-fetched
  candle inputs.
- **Tool wrappers co-located with calculator**: Follows the same pattern as `add-financial-data` where `#[tool]`
  wrappers live alongside the implementation code they delegate to. In this case, `tools.rs` imports calculation
  functions from the sibling submodules and delegates to them.

## Risks / Trade-offs

- **`kand` API stability**: v0.2 is relatively new. Mitigated by wrapping all calls behind internal functions
  so a future crate version change only affects `src/indicators/core_math.rs`.
- **Indicator lookback periods**: Some indicators (e.g., 200 SMA) require 200+ candles of history. If the
  requested date range yields fewer candles, those indicators return `None`. The Technical Analyst prompt
  instructs the LLM to note when indicators are unavailable.
- **Weekly smoothing trade-off**: Using weekly pivots and long-horizon clustering matches the system's long-term
  investing posture, but it will react more slowly to sharp short-term regime changes than a daily-trading formula.
- **Floating-point edge cases**: `kand` handles NaN propagation internally, but our wrapper adds an explicit
  validation layer for empty/zero-length inputs.

## Open Questions

- Should indicator periods (e.g., RSI 14, SMA 50/200) be configurable via `Config`, or hardcoded to match the
  prompt spec? (Current decision: hardcode to match the reference paper's defaults; configurability deferred
  to post-MVP.)
- Future improvement: should crypto-specific analysis become a separate capability layered on top of this OHLCV
  calculator, or should `technical-analysis` itself expand to cover crypto-native interpretation and on-chain metrics?
