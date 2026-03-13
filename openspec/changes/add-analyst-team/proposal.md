# Change: Add Analyst Team Agents

## Why

The Analyst Team is the sensory input layer of the TradingAgents pipeline (Phase 1). Without it, no market data enters
the LLM reasoning chain and all downstream phases (Researcher debate, Trader, Risk, Fund Manager) have nothing to
operate on. This proposal introduces the four parallel analyst agents — Fundamental, Sentiment, News, and Technical —
each implemented as a `rig` agent with typed tool bindings, a domain-specific system prompt, and structured output that
populates the corresponding `Option<T>` field on `TradingState`. The agents execute concurrently via `tokio::spawn`
using the fan-out pattern defined in the architect plan.

## What Changes

- Implement `FundamentalAnalyst` agent (`src/agents/analyst/fundamental.rs`) — uses `QuickThinking` tier, binds Finnhub
  fundamental/earnings/insider-transaction tools from `financial-data`, writes `FundamentalData` to
  `TradingState::fundamental_metrics`.
- Implement `SentimentAnalyst` agent (`src/agents/analyst/sentiment.rs`) — uses `QuickThinking` tier, consumes
  company-specific news from Finnhub and/or Yahoo Finance for MVP sentiment analysis, writes `SentimentData` to
  `TradingState::market_sentiment`. Direct Reddit/X ingestion is deferred.
- Implement `NewsAnalyst` agent (`src/agents/analyst/news.rs`) — uses `QuickThinking` tier, binds Finnhub market news
  and economic indicator tools, writes `NewsData` to `TradingState::macro_news`.
- Implement `TechnicalAnalyst` agent (`src/agents/analyst/technical.rs`) — uses `QuickThinking` tier, binds Yahoo
  Finance OHLCV tool from `financial-data` and indicator calculation tools from `technical-analysis`, writes
  `TechnicalData` to `TradingState::technical_indicators`.
- Wire the analyst module's public API through `src/agents/analyst/mod.rs`, exposing a `run_analyst_team` fan-out
  function and individual analyst entry points.
- Each agent records `AgentTokenUsage` (model ID, prompt/completion tokens, latency) for the `TokenUsageTracker`.
- Apply per-analyst 30-second timeout via `tokio::time::timeout` and follow the graceful degradation policy (1 analyst
  failure = continue, 2+ = abort).

## Impact

- Affected specs: `analyst-team` (new)
- Affected code: `src/agents/analyst/mod.rs` (fill in skeleton), `src/agents/analyst/fundamental.rs` (new),
  `src/agents/analyst/sentiment.rs` (new), `src/agents/analyst/news.rs` (new),
  `src/agents/analyst/technical.rs` (new)
- Dependencies: `add-project-foundation` (core types, error handling, config, rate-limiting, module stubs),
  `add-llm-providers` (provider factory, agent builder helper, retry-wrapped completions, ModelTier),
  `add-financial-data` (Finnhub and Yahoo Finance clients and rig tool wrappers),
  `add-technical-analysis` (kand indicator calculator and rig tool wrappers)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`), provider-owned files
  (`src/providers/*`), data-layer files (`src/data/*`), or indicator files (`src/indicators/*`)
