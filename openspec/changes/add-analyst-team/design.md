# Design for `add-analyst-team`

## Context

The foundation layer (`core-types`, `config`, `error-handling`, `rate-limiting`, `observability`), the provider layer
(`llm-providers`), the data layer (`financial-data`), and the indicator layer (`technical-analysis`) are all specified.
This change introduces the first agents in the pipeline — four analyst agents that consume market data through typed
tools and populate `TradingState` fields with structured outputs. The agents run concurrently during Phase 1 of the
5-phase execution graph and must tolerate partial failures gracefully.

**Stakeholders:** `add-graph-orchestration` (wraps analysts into `graph_flow::Task` fan-out), `add-researcher-debate`
(consumes analyst outputs from `TradingState`), `add-trader-agent` (reads all analyst data), `add-risk-management`
(reads analyst data indirectly via the trader proposal).

## Goals / Non-Goals

- **Goals:**
  - Implement four `rig` agents (Fundamental, Sentiment, News, Technical) each with a domain-specific system prompt
    derived from `docs/prompts.md`, typed tool bindings, and structured JSON output extraction.
  - Use the `QuickThinking` tier from `llm-providers` for all analyst agents.
  - Provide a `run_analyst_team` function that spawns all four analysts concurrently via `tokio::spawn`, collects
    results, applies the graceful degradation policy, and writes outputs to `TradingState` using per-field
    `Arc<RwLock<Option<T>>>` locking.
  - Record `AgentTokenUsage` for each analyst immediately after each LLM completion returns.
  - Enforce a per-analyst 30-second timeout via `tokio::time::timeout` (configurable through `Config.llm`).
  - Confine all implementation to `src/agents/analyst/` without modifying foundation, provider, data, or indicator
    files.

- **Non-Goals:**
  - Implementing the `graph_flow::Task` wrapper — belongs to `add-graph-orchestration`.
  - Implementing the debate loop or any downstream agent — belongs to respective agent changes.
  - Fetching data directly from APIs — analysts invoke tools that delegate to the `financial-data` and
    `technical-analysis` layers.
  - Direct Reddit/X social-platform ingestion — deferred to future improvements; the MVP Sentiment Analyst uses
    company-specific news from existing data sources.
  - Per-agent provider overrides — the MVP uses tier-level provider config only.

## Architectural Overview

```
src/agents/analyst/
├── mod.rs           ← Re-exports + run_analyst_team fan-out function
├── fundamental.rs   ← Fundamental Analyst agent
├── sentiment.rs     ← Sentiment Analyst agent
├── news.rs          ← News Analyst agent
└── technical.rs     ← Technical Analyst agent
```

### Agent Construction Pattern

Each analyst follows a uniform construction pattern:

1. Obtain a `QuickThinking` completion model from the provider factory.
2. Build a `rig` agent via the agent builder helper with:
   - A system prompt sourced from constants matching `docs/prompts.md`.
   - Typed tool bindings from `financial-data` or `technical-analysis`.
3. Invoke the agent via `prompt_with_retry` (one-shot prompt, no chat history needed for analysts).
4. Extract the structured output into the corresponding `core-types` data struct.
5. Record `AgentTokenUsage` (model ID, token counts from rig completion response, wall-clock latency).
6. Return `Result<T, TradingError>` where `T` is the agent's output data struct.

### Fan-Out Execution Pattern

```
run_analyst_team(state, config, providers, data_clients, rate_limiter)
  │
  ├─ tokio::spawn(fundamental_analyst) ──► Ok(FundamentalData) or Err(TradingError)
  ├─ tokio::spawn(sentiment_analyst)   ──► Ok(SentimentData)   or Err(TradingError)
  ├─ tokio::spawn(news_analyst)        ──► Ok(NewsData)        or Err(TradingError)
  └─ tokio::spawn(technical_analyst)   ──► Ok(TechnicalData)   or Err(TradingError)
      │
      ▼
  tokio::join! all handles
      │
      ▼
  Apply graceful degradation:
    - 0 failures: write all outputs to TradingState
    - 1 failure:  write available outputs, log warning, continue pipeline
    - 2+ failures: return TradingError::AnalystError (abort)
```

### Data Dependencies Per Analyst

| Analyst       | Tools Bound                                                      | Output Type       | TradingState Field          |
|---------------|------------------------------------------------------------------|-------------------|-----------------------------|
| Fundamental   | Finnhub fundamentals, earnings, insider transactions             | `FundamentalData` | `fundamental_metrics`       |
| Sentiment     | Finnhub news, Yahoo Finance news (company-specific)              | `SentimentData`   | `market_sentiment`          |
| News          | Finnhub market news, economic indicators                         | `NewsData`        | `macro_news`                |
| Technical     | Yahoo Finance OHLCV, kand batch/individual indicator calculators | `TechnicalData`   | `technical_indicators`      |

### Timeout and Degradation

- Each `tokio::spawn` task is wrapped in `tokio::time::timeout(Duration::from_secs(config.llm.analyst_timeout_secs))`.
- If a task times out, it produces a `TradingError::NetworkTimeout`.
- The fan-out function counts failures and applies the degradation policy before returning.
- Failed analysts have their corresponding `TradingState` field left as `None`.

### Token Accounting

Each analyst task measures wall-clock latency (start to completion return) and extracts token metadata from the `rig`
completion response. The resulting `AgentTokenUsage` is returned alongside the analyst output so the upstream
orchestrator can aggregate it into `PhaseTokenUsage` for "Phase 1: Analyst Team". When the underlying provider does not
expose authoritative token counts, the agent still records latency and model identity with documented unavailable-token
metadata per the `core-types` contract.

### System Prompts

System prompts are defined as `const &str` values within each analyst module, derived from `docs/prompts.md`. The
prompts contain two components:

1. **Goal/Instructions prompt** — the domain-specific analyst directive.
2. **ReAct/Collaboration base prompt** — the shared collaborative agent framework.

Runtime parameters (`{current_date}`, `{ticker}`, `{tool_names}`) are interpolated at agent construction time using
the `TradingState.asset_symbol`, `TradingState.target_date`, and the registered tool names.

## Key Decisions

- **One-shot prompt, not chat**: Analysts perform a single data-gathering + reasoning cycle. Unlike researchers who
  debate across rounds, analysts have no conversational history. This means `prompt_with_retry` (not
  `chat_with_retry`) is the correct invocation path.

- **Structured output via rig schema enforcement**: Each analyst's output must be a well-formed JSON struct matching
  the `core-types` definition. The provider layer's structured output validation catches malformed responses and
  surfaces `TradingError::SchemaViolation`, triggering retry via `prompt_with_retry`.

- **Tools are injected, not constructed**: Analyst agents receive pre-constructed tool objects from the data and
  indicator layers. This preserves the module boundary — analysts import tool types but never directly call Finnhub
  or kand APIs.

- **MVP Sentiment uses news-based analysis**: The Sentiment Analyst consumes the same company-specific news inputs
  available from the `financial-data` layer (Finnhub news, Yahoo Finance news). This avoids expanding the data layer
  for social-platform scraping while still providing a meaningful sentiment signal from available structured sources.

- **Fan-out function owns degradation logic**: The `run_analyst_team` function — not the orchestrator — applies the
  1-failure/2-failure policy. This keeps the degradation rule co-located with the agent team and testable in
  isolation without requiring a full `graph-flow` pipeline.

- **System prompts as module constants**: Prompts are embedded as string constants rather than loaded from external
  files. This makes them compile-time checked, version-controlled alongside the agent code, and avoids runtime file
  I/O or configuration complexity.

## Risks / Trade-offs

- **Prompt drift from reference paper**: The prompts are adapted from the Python reference (`docs/prompts.md`) but
  may need Rust-specific adjustments (e.g., tool name formatting, JSON schema instructions). Mitigation: integration
  tests with mocked LLM responses validate that the expected structured output schema is produced.

- **Sentiment quality in MVP**: News-based sentiment is less nuanced than social-media-informed sentiment. Mitigation:
  the PRD explicitly defers social-platform ingestion; the MVP Sentiment Analyst provides a baseline that can be
  enhanced later without changing the agent interface.

- **Tool execution latency**: Each analyst may make multiple tool calls (e.g., Fundamental Analyst fetches
  fundamentals + earnings + insider transactions). The 30-second timeout must accommodate the aggregate tool
  execution time including rate limiter waits. Mitigation: the timeout is configurable via `Config.llm`.

- **Token counting accuracy**: Depends on the provider exposing authoritative token metadata in the completion
  response. The `core-types` contract already handles unavailable counts gracefully.

## Open Questions

- Should the `run_analyst_team` function accept a list of analysts to run (allowing partial team composition for
  testing or degraded mode), or always run all four? Recommendation: always run all four in MVP; selective execution
  can be added as a future enhancement if needed for backtesting optimization.
