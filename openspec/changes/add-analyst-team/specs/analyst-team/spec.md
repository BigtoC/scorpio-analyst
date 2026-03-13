# `analyst-team` Capability

## ADDED Requirements

### Requirement: Fundamental Analyst Agent

The system MUST implement a Fundamental Analyst agent as a `rig` agent using the `QuickThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Fundamentals Analyst section), incorporating the target asset symbol
  and current date at construction time.
- Typed tool bindings for Finnhub fundamentals, earnings, and insider transaction retrieval from the `financial-data`
  capability.

The agent MUST invoke its tools to gather corporate financials (revenue growth, P/E ratios, liquidity ratios) and
insider transaction data, then synthesize the data into a structured `FundamentalData` output validated against the
`core-types` schema. The output MUST be written to `TradingState::fundamental_metrics`. The agent MUST record
`AgentTokenUsage` (agent name "Fundamental Analyst", model ID, prompt/completion/total tokens, wall-clock latency)
from the completion response.

#### Scenario: Successful Fundamental Analysis

- **WHEN** the Fundamental Analyst agent is invoked with a valid asset symbol and the Finnhub API returns data
  successfully
- **THEN** the agent populates `TradingState::fundamental_metrics` with a `FundamentalData` struct containing revenue
  growth, P/E ratio, liquidity ratio, and insider transaction data, and records `AgentTokenUsage` with model ID and
  token counts

#### Scenario: Finnhub Tool Returns Partial Data

- **WHEN** the Finnhub API returns partial data (e.g., missing insider transactions for a small-cap company)
- **THEN** the agent still produces a valid `FundamentalData` output with available fields populated and missing
  fields represented as `None`, rather than failing the entire analysis

#### Scenario: Schema Violation On LLM Output

- **WHEN** the LLM returns output that does not conform to the `FundamentalData` JSON schema
- **THEN** the provider layer raises `TradingError::SchemaViolation` and the retry policy retries the prompt up to
  the configured maximum attempts

### Requirement: Sentiment Analyst Agent

The system MUST implement a Sentiment Analyst agent as a `rig` agent using the `QuickThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Social Media Analyst section, adapted for news-based MVP), incorporating
  the target asset symbol and current date at construction time.
- Typed tool bindings for company-specific news retrieval from the `financial-data` capability (Finnhub news and/or
  Yahoo Finance news where available).

The MVP Sentiment Analyst MUST derive sentiment from company-specific news coverage rather than direct social-platform
ingestion (Reddit/X). The agent MUST analyze recent news to identify tone shifts, recurring themes, management/product
narratives, and event-driven sentiment, then synthesize findings into a structured `SentimentData` output. The output
MUST be written to `TradingState::market_sentiment`. The agent MUST record `AgentTokenUsage`.

#### Scenario: Successful News-Based Sentiment Analysis

- **WHEN** the Sentiment Analyst is invoked and the news retrieval tool returns recent company-specific articles
- **THEN** the agent populates `TradingState::market_sentiment` with a `SentimentData` struct containing normalized
  sentiment scores and source breakdown derived from the news data

#### Scenario: No Recent News Available

- **WHEN** the news retrieval tool returns an empty result set for the target company
- **THEN** the agent produces a valid `SentimentData` output indicating neutral or inconclusive sentiment with empty
  source collections, rather than failing

#### Scenario: Social Platform Data Is Not Used In MVP

- **WHEN** the Sentiment Analyst executes during the MVP
- **THEN** it does not attempt to access Reddit, X/Twitter, or other social media platforms, relying exclusively on
  structured news sources from the `financial-data` capability

### Requirement: News Analyst Agent

The system MUST implement a News Analyst agent as a `rig` agent using the `QuickThinking` model tier. The agent MUST be
constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (News Analyst section), incorporating the target asset symbol and
  current date at construction time.
- Typed tool bindings for Finnhub market news and economic indicator endpoints from the `financial-data` capability.

The agent MUST process breaking news and macroeconomic data to extract causal relationships relevant to the target
asset (e.g., geopolitical tensions, tariff impacts, interest rate commentary affecting supply chains or discount
rates). The output MUST be a structured `NewsData` struct written to `TradingState::macro_news`. The agent MUST record
`AgentTokenUsage`.

#### Scenario: Successful Macro News Analysis

- **WHEN** the News Analyst is invoked and Finnhub returns recent market news and economic indicators
- **THEN** the agent populates `TradingState::macro_news` with a `NewsData` struct containing articles, macro events,
  and identified causal relationships

#### Scenario: News Analyst Identifies Causal Chain

- **WHEN** news articles reference sector-specific events (e.g., semiconductor tariffs, interest rate changes)
- **THEN** the agent's `NewsData` output includes extracted causal relationships linking the macro event to the
  target asset's sector or supply chain

### Requirement: Technical Analyst Agent

The system MUST implement a Technical Analyst agent as a `rig` agent using the `QuickThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Market / Technical Analyst section), incorporating the target asset
  symbol and current date at construction time.
- Typed tool bindings for Yahoo Finance OHLCV retrieval from the `financial-data` capability and for indicator
  calculation functions (batch and individual) from the `technical-analysis` capability.

The agent MUST first retrieve historical OHLCV data via the OHLCV tool, then invoke indicator calculation tools to
compute the selected technical indicators. The LLM interprets the statistical outputs (RSI overbought/oversold
conditions, MACD crossovers, ATR volatility, support/resistance levels) but does not perform mathematical calculations
itself. The output MUST be a structured `TechnicalData` struct written to `TradingState::technical_indicators`. The
agent MUST record `AgentTokenUsage`. This agent's interpretation is designed for traditional OHLCV-based long-term
investing workflows; crypto-native analysis is deferred.

#### Scenario: Successful Technical Analysis With Full Indicator Suite

- **WHEN** the Technical Analyst is invoked and Yahoo Finance returns 200+ candles of OHLCV data
- **THEN** the agent populates `TradingState::technical_indicators` with a fully computed `TechnicalData` struct
  including RSI, MACD, ATR, Bollinger Bands, SMAs, EMA, VWMA, and support/resistance levels

#### Scenario: Technical Analyst Selects Indicators Via Prompt Names

- **WHEN** the Technical Analyst's prompt instructs it to select specific indicators using prompt-compatible names
  (e.g., `rsi`, `macd`, `close_50_sma`)
- **THEN** the agent invokes the named-indicator tool from `technical-analysis` using those exact names, and the tool
  returns the corresponding indicator values

#### Scenario: Insufficient OHLCV Data For Long-Period Indicators

- **WHEN** Yahoo Finance returns fewer than 200 candles for the requested date range
- **THEN** the batch indicator tool returns partial results (e.g., SMA 200 as `None`), and the agent notes the
  unavailability in its analysis output without failing

### Requirement: Analyst Team Fan-Out Execution

The system MUST provide a `run_analyst_team` function that executes all four analyst agents concurrently using
`tokio::spawn`. Each spawned task MUST be wrapped in `tokio::time::timeout` using the configured
`analyst_timeout_secs` (default 30 seconds). The function MUST collect all results and apply the graceful degradation
policy:

- If all four analysts succeed, write all outputs to their respective `TradingState` fields.
- If exactly one analyst fails (timeout, LLM error, or tool error), write the available outputs, log a warning
  identifying the failed analyst, and allow the pipeline to continue with partial data.
- If two or more analysts fail, return a `TradingError::AnalystError` indicating which analysts failed, aborting the
  current analysis cycle.

Failed analysts MUST have their corresponding `TradingState` field left as `None`. Successful analysts MUST write
their output using per-field locking (`Arc<RwLock<Option<T>>>`) to prevent contention during concurrent writes. The
function MUST return the collected `AgentTokenUsage` entries for all completed analysts (both successful and failed
before timeout) so the upstream orchestrator can aggregate them into a `PhaseTokenUsage` entry for Phase 1.

#### Scenario: All Four Analysts Succeed

- **WHEN** `run_analyst_team` is invoked and all four analysts complete within the timeout
- **THEN** all four `TradingState` fields (`fundamental_metrics`, `market_sentiment`, `macro_news`,
  `technical_indicators`) are populated, and four `AgentTokenUsage` entries are returned

#### Scenario: One Analyst Times Out

- **WHEN** `run_analyst_team` is invoked and the Sentiment Analyst exceeds the 30-second timeout while the other
  three complete successfully
- **THEN** `TradingState::market_sentiment` remains `None`, the other three fields are populated, a warning is
  logged identifying "Sentiment Analyst" as the timed-out agent, and the pipeline continues

#### Scenario: Two Analysts Fail

- **WHEN** `run_analyst_team` is invoked and both the Fundamental Analyst and the News Analyst fail
- **THEN** the function returns `TradingError::AnalystError` listing both failed agents, and the pipeline aborts
  the current analysis cycle

#### Scenario: Timeout Is Configurable

- **WHEN** `Config.llm.analyst_timeout_secs` is set to 60 seconds
- **THEN** each analyst task is allowed up to 60 seconds before being terminated

### Requirement: Agent Token Usage Recording

Each analyst agent MUST record an `AgentTokenUsage` entry immediately after the LLM completion call returns. The entry
MUST contain the agent's display name (e.g., "Fundamental Analyst"), the model ID used for the completion, and
wall-clock latency measured from prompt submission to response receipt. When the provider exposes authoritative
prompt/completion/total token counts, those MUST be recorded. When the provider does not expose authoritative counts,
the agent MUST preserve the documented unavailable-token representation from `core-types` rather than fabricating
counts.

#### Scenario: Token Usage Recorded With Full Provider Metadata

- **WHEN** the Fundamental Analyst completes a prompt using an OpenAI model that reports token counts in its response
- **THEN** the `AgentTokenUsage` entry contains agent name "Fundamental Analyst", the OpenAI model ID, accurate
  prompt/completion/total token counts, and measured wall-clock latency

#### Scenario: Token Usage Recorded When Counts Unavailable

- **WHEN** an analyst completes a prompt using a provider that does not report authoritative token counts
- **THEN** the `AgentTokenUsage` entry still contains agent name, model ID, and wall-clock latency, with token
  count fields using the documented unavailable representation

### Requirement: Analyst Module Boundary

This capability's implementation MUST remain limited to analyst agent concerns within `src/agents/analyst/mod.rs`,
`src/agents/analyst/fundamental.rs`, `src/agents/analyst/sentiment.rs`, `src/agents/analyst/news.rs`, and
`src/agents/analyst/technical.rs`. It MUST re-export the `run_analyst_team` function and individual analyst types
from `src/agents/analyst/mod.rs` for consumption by the downstream `add-graph-orchestration` change. The analyst
module MUST NOT modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`, `src/rate_limit.rs`),
provider-owned files (`src/providers/*`), data-layer files (`src/data/*`), or indicator files (`src/indicators/*`).

#### Scenario: Downstream Orchestrator Import Path

- **WHEN** the downstream `add-graph-orchestration` change imports the analyst team
- **THEN** it uses `use scorpio_analyst::agents::analyst::{run_analyst_team, ...}` and receives the fan-out function
  and analyst types through the agent module path

#### Scenario: No Foundation Or Upstream File Modifications

- **WHEN** the analyst team module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`,
  `src/rate_limit.rs`, the provider-owned `src/providers/*`, the data-layer `src/data/*`, and the indicator
  `src/indicators/*` files all remain unmodified, as all dependencies and module declarations were pre-declared by
  `add-project-foundation`
