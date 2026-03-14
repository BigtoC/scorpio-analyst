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
  function that orchestrates Bullish -> Bearish rounds for `max_debate_rounds` iterations and then invokes the Debate
  Moderator once to write the final `consensus_summary`. Workflow-level `graph_flow` loop routing remains the
  responsibility of `add-graph-orchestration`.
- Each agent records `AgentTokenUsage` (model ID, prompt/completion tokens, latency, and availability metadata) for the
  `TokenUsageTracker`. The debate loop produces per-invocation `AgentTokenUsage` entries so the orchestrator can build
  per-round `PhaseTokenUsage` records.
- The debate loop uses `rig` chat history to enable each researcher to directly address the counterpart's prior
  arguments, building a nuanced multi-dimensional evaluation across rounds.

## Impact

- Affected specs: `researcher-debate` (new)
- Affected code: `src/agents/researcher/mod.rs` (fill in skeleton), `src/agents/researcher/bullish.rs` (new),
  `src/agents/researcher/bearish.rs` (new), `src/agents/researcher/moderator.rs` (new), and optional private helper
  modules under `src/agents/researcher/` if needed to share prompt-formatting or token-accounting logic
- Dependencies: `add-project-foundation` (core types including `DebateMessage`, `TradingState`, error handling,
  config for `max_debate_rounds`), `add-llm-providers` (provider factory, agent builder helper, `DeepThinking` tier,
  chat-with-retry helper)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`), provider-owned files
  (`src/providers/*`) except for the approved cross-owner addition below, data-layer files (`src/data/*`), indicator
  files (`src/indicators/*`), or analyst-owned files (`src/agents/analyst/*`)
- Downstream consumers: `add-graph-orchestration` (wraps researchers into `graph_flow::Task` cyclic pattern),
  `add-trader-agent` (reads `consensus_summary` produced by the Debate Moderator)

## Cross-Owner Changes

- `src/providers/factory.rs` — owner: `add-llm-providers`.
  Justification: the Bullish and Bearish researchers use history-aware chat turns and must also record
  `AgentTokenUsage` for each round. The current provider API exposes one-shot usage details for prompts, but the chat
  path only returns plain text. `add-researcher-debate` therefore needs a minimal provider-layer addition such as a
  `chat_with_retry_details` helper (and any supporting `LlmAgent` chat-details method) so debate rounds can capture
  authoritative usage metadata without reimplementing provider-specific chat logic inside the researcher module.
