# Design for `add-risk-management`

## Context

The foundation layer (`core-types`, `config`, `error-handling`) and the provider layer (`llm-providers`) are both
specified and provide the types, configuration, and LLM invocation helpers this change depends on. The Trader Agent
(`add-trader-agent`) populates `TradingState::trader_proposal` with a structured `TradeProposal` during Phase 3.
This change introduces Phase 4 — a cyclic risk discussion among Aggressive, Conservative, and Neutral risk persona
agents, moderated by a Risk Moderator, that evaluates the `TradeProposal` against the analyst data and produces
structured `RiskReport` outputs and a synthesized `risk_discussion_history` for the Fund Manager.

The risk team follows the same cyclic debate pattern established by `add-researcher-debate`, with key differences:
three participants instead of two, structured JSON output (`RiskReport`) for the persona agents instead of plain text,
and evaluation of a concrete `TradeProposal` rather than raw analyst data.

**Stakeholders:** `add-graph-orchestration` (wraps risk discussion into `graph_flow::Task` cyclic nodes and owns
`NextAction`/`GoBack` routing), `add-fund-manager` (consumes `RiskReport` objects and `risk_discussion_history`).

## Goals / Non-Goals

- **Goals:**
    - Implement three risk persona `rig` agents (Aggressive, Conservative, Neutral) each with a domain-specific system
      prompt derived from `docs/prompts.md` and structured `RiskReport` JSON output.
    - Implement a Risk Moderator `rig` agent that synthesizes the three risk perspectives into a plain-text discussion
      summary, explicitly noting whether Conservative and Neutral both flag a material violation.
    - Use the `DeepThinking` tier from `llm-providers` for all risk agents.
    - Provide a `run_risk_discussion` function that executes the cyclic discussion loop: for each round, invoke all
      three risk persona agents sequentially, append their structured reports and plain-text discussion entries to
      `TradingState`, and after the final round invoke the Risk Moderator to produce the final synthesis.
    - The discussion runner MUST respect `Config.llm.max_risk_rounds` (default 2) and terminate deterministically.
    - Each agent invocation records `AgentTokenUsage` with per-round granularity so the upstream orchestrator can build
      per-round `PhaseTokenUsage` entries.
    - Maintain chat history across rounds so each risk agent can directly address the other agents' prior positions.
    - Confine all implementation to `src/agents/risk/` without modifying foundation, provider, or other agent-owned
      files, while still allowing private helper modules inside `src/agents/risk/` when needed to reduce duplication.
    - Use the provider layer's existing `prompt_with_retry_details` path (from `add-llm-providers`, extended by the
      `add-researcher-debate` cross-owner addition) for structured JSON extraction with usage metadata.

- **Non-Goals:**
    - Implementing the `graph_flow::Task` wrapper or `NextAction` routing — belongs to `add-graph-orchestration`.
    - Implementing the Fund Manager's decision logic — belongs to `add-fund-manager`.
    - Modifying `RiskReport`, `RiskLevel`, `DebateMessage`, or `TradingState` — the existing `core-types` definitions
      are sufficient.
    - Tool bindings — risk agents are reasoning agents that interpret analyst data and the trade proposal, not
      tool-calling agents.
    - Per-agent provider overrides — the MVP uses tier-level provider config only.

## Architectural Overview

```
src/agents/risk/
├── mod.rs           <- Re-exports + run_risk_discussion cyclic loop function
├── aggressive.rs    <- Aggressive Risk Agent
├── conservative.rs  <- Conservative Risk Agent
├── neutral.rs       <- Neutral Risk Agent
└── moderator.rs     <- Risk Moderator agent
```

### Agent Construction Pattern

Each risk persona agent follows a uniform construction pattern:

1. Obtain a `DeepThinking` completion model from the provider factory.
2. Build a `rig` agent via the agent builder helper with a system prompt sourced from constants matching
   `docs/prompts.md`. No tool bindings are attached; risk agents are pure reasoning agents.
3. Serialize the `TradeProposal` and analyst outputs from `TradingState` (fundamental, technical, sentiment, news data)
   into prompt context, along with the other risk agents' latest views and the accumulated risk discussion history.
4. Use the provider layer's `prompt_with_retry_details` for structured `RiskReport` JSON extraction, or
   `chat_with_retry_details` for history-aware rounds. The chosen path must return both response content and usage
   metadata.
5. Validate the returned `RiskReport` JSON against the schema before writing to `TradingState`, rejecting malformed
   output with `TradingError::SchemaViolation`.
6. Record `AgentTokenUsage` (model ID, token counts when available, `token_counts_available`, wall-clock latency).
7. Return `Result<(RiskReport, AgentTokenUsage), TradingError>`.

### Cyclic Risk Discussion Loop

```
run_risk_discussion(state, config, providers)
  │
  │  for round in 1..=max_risk_rounds:
  │    │
  │    ├─ Aggressive Risk Agent
  │    │   ├─ Input: trade proposal + analyst data + risk history + conservative/neutral latest
  │    │   ├─ Output: RiskReport { risk_level: Aggressive, ... }
  │    │   ├─ Write to TradingState::aggressive_risk_report
  │    │   └─ Append DebateMessage { role: "aggressive_risk", content: assessment } to risk_discussion_history
  │    │
  │    ├─ Conservative Risk Agent
  │    │   ├─ Input: trade proposal + analyst data + risk history + aggressive/neutral latest
  │    │   ├─ Output: RiskReport { risk_level: Conservative, ... }
  │    │   ├─ Write to TradingState::conservative_risk_report
  │    │   └─ Append DebateMessage { role: "conservative_risk", content: assessment } to risk_discussion_history
  │    │
  │    └─ Neutral Risk Agent
  │        ├─ Input: trade proposal + analyst data + risk history + aggressive/conservative latest
  │        ├─ Output: RiskReport { risk_level: Neutral, ... }
  │        ├─ Write to TradingState::neutral_risk_report
  │        └─ Append DebateMessage { role: "neutral_risk", content: assessment } to risk_discussion_history
  │
  │  After all rounds:
  │    └─ Risk Moderator (prompt_with_retry_details)
  │        ├─ Input: trade proposal + all three latest RiskReports + full risk_discussion_history + analyst data
  │        ├─ Output: plain-text discussion synthesis
  │        └─ Append DebateMessage { role: "risk_moderator", content: synthesis } to risk_discussion_history
  │
  └─ Return Vec<AgentTokenUsage> for all invocations
```

### Context Injection Strategy

Risk agents receive serialized data in their system prompt or as prompt context:

| Placeholder                | Source                                                    |
|----------------------------|-----------------------------------------------------------|
| `{trader_proposal}`       | `serde_json::to_string(&state.trader_proposal)`          |
| `{fundamental_report}`    | `serde_json::to_string(&state.fundamental_metrics)`      |
| `{technical_report}`      | `serde_json::to_string(&state.technical_indicators)`     |
| `{sentiment_report}`      | `serde_json::to_string(&state.market_sentiment)`         |
| `{news_report}`           | `serde_json::to_string(&state.macro_news)`               |
| `{risk_history}`          | Formatted from `state.risk_discussion_history`           |
| `{aggressive_response}`   | `serde_json::to_string(&state.aggressive_risk_report)`   |
| `{conservative_response}` | `serde_json::to_string(&state.conservative_risk_report)` |
| `{neutral_response}`      | `serde_json::to_string(&state.neutral_risk_report)`      |
| `{ticker}`                | `state.asset_symbol`                                      |
| `{current_date}`          | `state.target_date`                                       |
| `{past_memory_str}`       | Empty string for MVP (memory system deferred)            |

Missing analyst outputs (from graceful degradation) are serialized as `"null"` — the risk agent prompts explicitly
handle missing data per `docs/prompts.md`.

### Output Strategy: Structured JSON vs. Plain Text

Risk persona agents (Aggressive, Conservative, Neutral) produce **structured `RiskReport` JSON** because the Fund
Manager needs machine-readable risk assessments for its deterministic fallback rule (reject if Conservative + Neutral
both flag violation). The `RiskReport` is extracted using `rig`'s structured output extraction with
`prompt_with_retry_details`.

The Risk Moderator produces **plain text** because its output is a human-readable discussion synthesis stored as a
`DebateMessage.content` entry in `risk_discussion_history`, matching the runtime state model.

### Output Validation

- `RiskReport` JSON must be validated against the schema: `risk_level` must match the agent's persona
  (`Aggressive`, `Conservative`, or `Neutral`), `assessment` must be non-empty, `recommended_adjustments` must be
  a valid array, and `flags_violation` must be a valid boolean.
- Risk moderator plain-text output must be rejected if it contains disallowed control characters or exceeds the
  module's documented bounded-summary policy, returning `TradingError::SchemaViolation`.
- If implementation benefit justifies it, the risk module may add a private `common.rs` helper under
  `src/agents/risk/` mirroring the researcher team's local validation/token-accounting helpers.

### Chat History Management

Each risk persona agent maintains a chat history across rounds to enable cross-argument exchange:

- **Round 1**: Each agent receives the `TradeProposal`, analyst data, and empty risk history. The agents execute
  sequentially (Aggressive -> Conservative -> Neutral) so later agents in round 1 can see the earlier agents'
  reports.
- **Round 2+**: Each agent receives the updated risk discussion history including all prior round entries, plus the
  other agents' latest `RiskReport` outputs. Chat history accumulates across rounds.
- **Risk Moderator**: Uses a single one-shot prompt (not chat) since it evaluates the complete discussion history
  at once after all rounds complete.

### Token Accounting

Each agent invocation (3 per round for Aggressive + Conservative + Neutral, plus 1 for the Moderator) records
`AgentTokenUsage`. `AgentTokenUsage.token_counts_available` distinguishes provider-reported counts from unavailable
metadata. The `run_risk_discussion` function returns the full `Vec<AgentTokenUsage>` grouped by invocation order so
the upstream orchestrator can create per-round `PhaseTokenUsage` entries (e.g., "Risk Discussion Round 1",
"Risk Discussion Round 2", "Risk Discussion Moderation").

## Key Decisions

- **Sequential within rounds, not parallel**: All three risk agents execute sequentially within each round
  (Aggressive -> Conservative -> Neutral) rather than in parallel. This is because the prompts reference the other
  agents' latest views (`{aggressive_response}`, `{conservative_response}`, `{neutral_response}`), and sequential
  execution within a round allows later agents to see earlier agents' output from the same round. This matches the
  PRD's cyclic debate pattern for the risk team.

- **Structured JSON for persona agents, plain text for moderator**: Persona agents return `RiskReport` JSON because
  the Fund Manager uses `flags_violation` fields programmatically. The Risk Moderator returns plain text because
  its role is synthesis, not structured assessment.

- **Reports overwrite on each round**: Each round's `RiskReport` overwrites the previous round's report in the
  `TradingState` fields (`aggressive_risk_report`, etc.). Only the final round's reports are relevant for the Fund
  Manager. The `risk_discussion_history` preserves the full discussion trail for auditability.

- **Risk Moderator is one-shot, not chat**: The Risk Moderator evaluates the complete discussion after all rounds
  finish. It uses `prompt_with_retry_details` (not chat) because it has no prior conversation to continue.

- **No tool bindings**: Risk agents interpret analyst data and the trade proposal injected via prompt context. They
  do not call external APIs or computation tools, consistent with the PRD.

- **System prompts as module constants**: Same pattern as `add-analyst-team` and `add-researcher-debate` — prompts
  embedded as `const &str` values, compile-time checked, version-controlled alongside agent code.

- **Reuse existing provider helpers**: Unlike `add-researcher-debate` which required a cross-owner addition to
  `src/providers/factory.rs`, this change reuses the existing `prompt_with_retry_details` and
  `chat_with_retry_details` helpers already available from the provider layer. No cross-owner changes needed.

## Risks / Trade-offs

- **Prompt length growth**: Each round adds 3 `RiskReport` serializations plus discussion entries to the context.
  With `max_risk_rounds = 2`, the final round sees 3 prior reports + accumulated history. Deep-thinking models
  handle this well, but cost/latency increases with rounds. Mitigation: `max_risk_rounds` defaults to 2 (lower than
  researcher debate's 3), balancing quality vs. cost.

- **Sequential latency**: The risk phase has 3 agents per round (vs. 2 for researchers), so with 2 rounds that's
  7 sequential LLM calls (6 persona + 1 moderator) on the deep-thinking tier. Mitigation: the default 2 rounds
  (vs. researcher's 3) keeps total calls comparable.

- **RiskReport schema violations**: Deep-thinking models generally produce valid JSON, but malformed output is
  possible. Mitigation: structured output extraction with schema validation; retry on schema violation up to the
  configured retry limit.

- **Missing TradeProposal**: If Phase 3 failed to produce a `TradeProposal`, the risk discussion cannot proceed
  meaningfully. Mitigation: `run_risk_discussion` should return an error if `trader_proposal` is `None`.

- **Token counting accuracy**: Same risk as researcher team — depends on provider exposing authoritative token
  metadata. Handled by the existing `core-types` unavailable-token representation.

## Open Questions

- Should risk persona agents run in parallel within each round (fan-out) to reduce latency, with only the
  inter-round dependency being sequential? This would prevent later agents in a round from seeing earlier agents'
  output within the same round, but would reduce wall-clock time. Recommendation: defer to future enhancement.
  The MVP uses sequential execution for maximum context quality, matching the PRD's description of a cyclic
  discussion. Parallel fan-out within rounds can be explored post-MVP if latency is a concern.
