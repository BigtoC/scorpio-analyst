# Tasks for `add-analyst-team`

## Prerequisites

- [ ] `add-project-foundation` is complete (core types, error handling, config, rate-limiting, module stubs)
- [ ] `add-llm-providers` is complete (provider factory, agent builder helper, retry-wrapped completions, ModelTier)
- [ ] `add-financial-data` is complete (Finnhub and Yahoo Finance clients and rig tool wrappers)
- [ ] `add-technical-analysis` is complete (kand indicator calculator and rig tool wrappers)

## 1. Fundamental Analyst Agent (`src/agents/analyst/fundamental.rs`)

- [x] 1.1 Define the Fundamental Analyst system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Fundamentals Analyst section), with placeholders for `{current_date}`, `{ticker}`, and `{tool_names}`
- [x] 1.2 Implement `FundamentalAnalyst` struct with a constructor that accepts provider factory references,
      pre-constructed Finnhub tool objects (fundamentals, earnings, insider transactions), and runtime parameters
      (asset symbol, target date)
- [x] 1.3 Implement `run(&self) -> Result<(FundamentalData, AgentTokenUsage), TradingError>` that constructs a
      `rig` agent via the agent builder helper with the system prompt and Finnhub tools, invokes
      `prompt_with_retry`, extracts `FundamentalData` from the structured output, and records `AgentTokenUsage`
- [x] 1.4 Write unit tests with mocked LLM responses verifying correct `FundamentalData` extraction
- [x] 1.5 Write unit tests verifying `AgentTokenUsage` is recorded with correct agent name and model ID
- [x] 1.6 Write unit tests for `TradingError::SchemaViolation` when LLM returns malformed JSON

## 2. Sentiment Analyst Agent (`src/agents/analyst/sentiment.rs`)

- [x] 2.1 Define the Sentiment Analyst system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Social Media Analyst section, modified for news-based MVP), with placeholders for runtime parameters
- [x] 2.2 Implement `SentimentAnalyst` struct with a constructor that accepts provider factory references,
      pre-constructed news tool objects (Finnhub news), and runtime parameters
- [x] 2.3 Implement `run(&self) -> Result<(SentimentData, AgentTokenUsage), TradingError>` that constructs a
      `rig` agent via the agent builder helper, invokes `prompt_with_retry`, extracts `SentimentData`, and
      records `AgentTokenUsage`
- [x] 2.4 Write unit tests with mocked LLM responses verifying correct `SentimentData` extraction from news inputs
- [x] 2.5 Write unit tests verifying the agent does not attempt social-platform access
- [x] 2.6 Write unit tests for empty news input producing valid neutral `SentimentData`

## 3. News Analyst Agent (`src/agents/analyst/news.rs`)

- [x] 3.1 Define the News Analyst system prompt as a `const &str`, adapted from `docs/prompts.md` (News Analyst
      section), with placeholders for runtime parameters
- [x] 3.2 Implement `NewsAnalyst` struct with a constructor that accepts provider factory references,
      pre-constructed Finnhub news tool objects, and runtime parameters
- [x] 3.3 Implement `run(&self) -> Result<(NewsData, AgentTokenUsage), TradingError>` that constructs a `rig`
      agent via the agent builder helper, invokes `prompt_with_retry`, extracts `NewsData`, and records
      `AgentTokenUsage`
- [x] 3.4 Write unit tests with mocked LLM responses verifying correct `NewsData` extraction with causal
      relationships
- [x] 3.5 Write unit tests for `AgentTokenUsage` recording

## 4. Technical Analyst Agent (`src/agents/analyst/technical.rs`)

- [x] 4.1 Define the Technical Analyst system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Market / Technical Analyst section), with placeholders for runtime parameters
- [x] 4.2 Implement `TechnicalAnalyst` struct with a constructor that accepts provider factory references,
      the `financial-data` client (for direct OHLCV fetching), and runtime parameters
- [x] 4.3 Implement `run(&self) -> Result<(TechnicalData, AgentTokenUsage), TradingError>` that first retrieves
      OHLCV data via the `financial-data` client, instantiates indicator calculation tool objects with the data,
      constructs a `rig` agent via the agent builder helper with the indicator tools, invokes `prompt_with_retry`,
      extracts `TechnicalData`, and records `AgentTokenUsage`
- [x] 4.4 Write unit tests with mocked LLM responses verifying correct `TechnicalData` extraction including RSI,
      MACD, ATR, support/resistance
- [x] 4.5 Write unit tests verifying the agent uses prompt-compatible indicator names (`rsi`, `macd`, etc.)
- [x] 4.6 Write unit tests for partial indicator results when OHLCV data is insufficient

## 5. Fan-Out Execution (`src/agents/analyst/mod.rs`)

- [x] 5.1 Implement `run_analyst_team` function that spawns all four analysts concurrently via `tokio::spawn`,
      each wrapped in `tokio::time::timeout(Duration::from_secs(config.llm.analyst_timeout_secs))`
- [x] 5.2 Implement result collection via `tokio::join!` or equivalent, collecting
      `Result<(T, AgentTokenUsage), TradingError>` from each task
- [x] 5.3 Implement graceful degradation logic: count failures, apply 1-failure/2-failure policy, write
      successful outputs to `TradingState` using per-field `Arc<RwLock<Option<T>>>` locking
- [x] 5.4 Return collected `Vec<AgentTokenUsage>` for all completed analysts alongside the (possibly partial)
      `TradingState` updates
- [x] 5.5 Re-export `run_analyst_team` and individual analyst types from `src/agents/analyst/mod.rs`
- [x] 5.6 Write unit test: all four analysts succeed, verify all four `TradingState` fields populated
- [x] 5.7 Write unit test: one analyst times out, verify three fields populated, one `None`, warning logged
- [x] 5.8 Write unit test: two analysts fail, verify `TradingError::AnalystError` returned with both agent names
- [x] 5.9 Write unit test: configurable timeout (60s) is respected

## 6. Integration Tests

- [ ] 6.1 Write integration test: construct all four analysts with mocked provider and mocked data tools, run
      `run_analyst_team`, verify all `TradingState` fields populated with expected data types
- [ ] 6.2 Write integration test: simulate one analyst failure (mocked LLM error), verify graceful degradation
      and partial `TradingState` population
- [ ] 6.3 Write integration test: verify `AgentTokenUsage` entries are collected for all completed analysts

## 7. Documentation and CI

- [x] 7.1 Add inline doc comments (`///`) for all public types and functions in `mod.rs`, `fundamental.rs`,
      `sentiment.rs`, `news.rs`, and `technical.rs`
- [x] 7.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [x] 7.3 Ensure `cargo fmt -- --check` passes
- [x] 7.4 Ensure `cargo test` passes all new and existing tests

## 9. Analyst Tool-Calling Refactor

- [x] 9.1 Refactor `FundamentalAnalyst::run` to use `build_agent_with_tools` with `GetFundamentals`,
      `GetEarnings`, and `GetInsiderTransactions` tools; remove pre-fetch and serialised-context approach
- [x] 9.2 Refactor `SentimentAnalyst::run` to use `build_agent_with_tools` with `GetNews` tool; remove
      pre-fetch and serialised-context approach
- [x] 9.3 Refactor `NewsAnalyst::run` to use `build_agent_with_tools` with `GetNews` tool; remove
      pre-fetch and serialised-context approach
- [x] 9.4 Refactor `TechnicalAnalyst::run` to use `build_agent_with_tools` with `GetOhlcv`,
      `CalculateAllIndicators`, `CalculateRsi`, `CalculateMacd`, `CalculateAtr`, `CalculateBollingerBands`,
      and `CalculateIndicatorByName` tools; remove OHLCV pre-fetch, in-Rust indicator computation,
      and `format_indicator_context` helper; keep `derive_start_date` for prompt construction
- [x] 9.5 Ensure `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt -- --check` all pass
- [x] 9.6 Update all documentation and spec files to reflect tool-calling workflow: `PRD.md`, `docs/prompts.md`,
      `openspec/changes/add-analyst-team/design.md`, `openspec/changes/add-analyst-team/proposal.md`,
      and `openspec/changes/add-analyst-team/specs/analyst-team/spec.md`
