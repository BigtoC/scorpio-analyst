# Change: Add Risk Management Team

## Why

The Risk Management Team is Phase 4 of the TradingAgents pipeline. Without it, the Trader Agent's `TradeProposal`
passes directly to the Fund Manager without adversarial scrutiny from multiple risk perspectives, defeating the
capital-preservation guarantees central to the framework's superior risk-adjusted performance (0.91% max drawdown).
This proposal introduces Aggressive, Conservative, and Neutral risk persona agents plus a Risk Moderator — implemented
as `rig` agents using the `DeepThinking` model tier — that engage in a configurable multi-round cyclic discussion to
produce structured `RiskReport` outputs and a synthesized `risk_discussion_history` for downstream review by the
Fund Manager.

## What Changes

- Implement `AggressiveRiskAgent` (`src/agents/risk/aggressive.rs`) — uses `DeepThinking` tier, evaluates the
  `TradeProposal` against analyst data and other risk agents' latest views, produces a structured `RiskReport` JSON
  with `risk_level = Aggressive`. Advocates for upside capture while identifying genuine risk controls.
- Implement `ConservativeRiskAgent` (`src/agents/risk/conservative.rs`) — uses `DeepThinking` tier, evaluates the
  proposal from a Maximum Drawdown and capital-preservation perspective, produces a `RiskReport` with
  `risk_level = Conservative`. Actively vetoes trades exhibiting overbought conditions, macroeconomic uncertainty,
  or high beta.
- Implement `NeutralRiskAgent` (`src/agents/risk/neutral.rs`) — uses `DeepThinking` tier, functions as the moderating
  force optimizing the Sharpe Ratio by balancing aggressive upside against conservative downside protections, produces
  a `RiskReport` with `risk_level = Neutral`.
- Implement `RiskModerator` agent (`src/agents/risk/moderator.rs`) — uses `DeepThinking` tier, synthesizes the three
  risk perspectives into a concise plain-text discussion summary. Explicitly notes whether Conservative and Neutral
  both flag a material violation (the Fund Manager's deterministic rejection rule).
- Risk persona agents maintain internal multi-round chat history via `chat_with_retry_details`, then locally
  deserialize the returned raw JSON string into `RiskReport` and validate it. Validation covers persona/risk-level
  matching plus bounded text validation for `assessment` and each `recommended_adjustments` entry. The provider layer
  currently exposes typed one-shot prompting, but not typed chat.
- Wire the risk module's public API through `src/agents/risk/mod.rs`, exposing a `run_risk_discussion` function that
  orchestrates the cyclic discussion for `max_risk_rounds` iterations and then invokes the Risk Moderator once.
  Workflow-level `graph_flow` phase wiring remains the responsibility of `add-graph-orchestration`.
- Execute Aggressive -> Conservative -> Neutral sequentially within each discussion round so later agents can react to
  earlier same-round outputs. This proposal implements the cyclic-discussion half of the Phase 4 design; it does not
  add a same-round fan-out that would prevent cross-examination.
- Each agent records `AgentTokenUsage` (model ID, prompt/completion tokens, latency, and availability metadata) for the
  `TokenUsageTracker`. The risk loop produces per-invocation `AgentTokenUsage` entries so the orchestrator can build
  per-round `PhaseTokenUsage` records.
- The risk discussion loop uses `rig` chat history to enable each risk agent to directly address the other agents'
  arguments across rounds, building a progressively refined risk assessment.
- Prompt-bound `TradeProposal`, analyst data, and risk-history context are sanitized before LLM injection: missing
  analyst inputs remain explicit as `"null"`, injected context is treated as untrusted, secret-like substrings are
  redacted, and discussion-history context is bounded to control prompt growth.

## Impact

- Affected specs: `risk-management` (new)
- Affected code: `src/agents/risk/mod.rs` (fill in skeleton), `src/agents/risk/aggressive.rs` (new),
  `src/agents/risk/conservative.rs` (new), `src/agents/risk/neutral.rs` (new),
  `src/agents/risk/moderator.rs` (new), and optional private helper modules under `src/agents/risk/` if needed
  to share prompt-formatting, output validation, or token-accounting logic
- Dependencies: `add-project-foundation` (core types including `RiskReport`, `RiskLevel`, `DebateMessage`,
  `TradingState`, error handling, config for `max_risk_rounds`), `add-llm-providers` (provider factory, agent builder
  helper, `DeepThinking` tier, `prompt_with_retry_details` and `chat_with_retry_details` helpers)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`), provider-owned files
  (`src/providers/*`), data-layer files (`src/data/*`), indicator files (`src/indicators/*`), analyst-owned files
  (`src/agents/analyst/*`), researcher-owned files (`src/agents/researcher/*`), or trader-owned files
  (`src/agents/trader/*`)
- Downstream consumers: `add-graph-orchestration` (wraps risk agents into `graph_flow::Task` cyclic pattern),
  `add-fund-manager` (reads `RiskReport` objects and `risk_discussion_history` produced by the Risk Moderator)
