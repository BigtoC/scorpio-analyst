# Change: Add Researcher Debate Team

## Why

The Researcher Team is Phase 2 of the TradingAgents pipeline. Without it, the raw analyst outputs pass directly to
the Trader Agent without adversarial cross-examination, creating confirmation bias and reducing decision quality. This
proposal introduces the Bullish Researcher, Bearish Researcher, and Debate Moderator agents — implemented as `rig`
agents using the `DeepThinking` model tier — that engage in a configurable multi-round cyclic debate to produce a
balanced `consensus_summary` for downstream synthesis by the Trader Agent.

## What Changes

- Implement `BullishResearcher` agent (`src/agents/researcher/bullish.rs`) — uses `DeepThinking` tier, receives
  serialized analyst outputs and debate history as context, produces a plain-text bullish argument appended to
  `TradingState::debate_history` as a `DebateMessage`.
- Implement `BearishResearcher` agent (`src/agents/researcher/bearish.rs`) — uses `DeepThinking` tier, receives
  serialized analyst outputs and debate history as context, produces a plain-text bearish counter-argument appended
  to `TradingState::debate_history` as a `DebateMessage`.
- Implement `DebateModerator` agent (`src/agents/researcher/moderator.rs`) — uses `DeepThinking` tier, evaluates
  completed debate rounds, selects the prevailing perspective, and writes a structured `consensus_summary` to
  `TradingState::consensus_summary`.
- Wire the researcher module's public API through `src/agents/researcher/mod.rs`, exposing a `run_researcher_debate`
  function that orchestrates the cyclic Bullish -> Bearish -> Moderator loop for `max_debate_rounds` iterations.
- Each agent records `AgentTokenUsage` (model ID, prompt/completion tokens, latency) for the `TokenUsageTracker`.
  The cyclic loop produces per-round `AgentTokenUsage` entries so the orchestrator can build per-round
  `PhaseTokenUsage` records.
- The debate loop uses `rig` chat history to enable each researcher to directly address the counterpart's prior
  arguments, building a nuanced multi-dimensional evaluation across rounds.

## Impact

- Affected specs: `researcher-debate` (new)
- Affected code: `src/agents/researcher/mod.rs` (fill in skeleton), `src/agents/researcher/bullish.rs` (new),
  `src/agents/researcher/bearish.rs` (new), `src/agents/researcher/moderator.rs` (new)
- Dependencies: `add-project-foundation` (core types including `DebateMessage`, `TradingState`, error handling,
  config for `max_debate_rounds`), `add-llm-providers` (provider factory, agent builder helper, `DeepThinking` tier,
  chat-with-retry helper)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`), provider-owned files
  (`src/providers/*`), data-layer files (`src/data/*`), indicator files (`src/indicators/*`), or analyst-owned files
  (`src/agents/analyst/*`)
- Downstream consumers: `add-graph-orchestration` (wraps researchers into `graph_flow::Task` cyclic pattern),
  `add-trader-agent` (reads `consensus_summary` produced by the Debate Moderator)
