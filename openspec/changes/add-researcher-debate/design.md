# Design for `add-researcher-debate`

## Context

The foundation layer (`core-types`, `config`, `error-handling`) and the provider layer (`llm-providers`) are both
specified and provide the types, configuration, and LLM invocation helpers this change depends on. The analyst team
(`add-analyst-team`) populates `TradingState` with four structured analyst outputs during Phase 1. This change
introduces Phase 2 — a cyclic adversarial debate between Bullish and Bearish researchers, moderated by a Debate
Moderator, that synthesizes the analyst outputs into a balanced `consensus_summary` for the Trader Agent.

Unlike the analyst team (which uses one-shot structured JSON output and parallel fan-out), the researcher team uses
multi-turn chat history and sequential cyclic execution. This is the first agent layer to exercise the `DeepThinking`
model tier and the `chat_with_retry` invocation path from `llm-providers`.

**Stakeholders:** `add-graph-orchestration` (wraps debate into `graph_flow::Task` cyclic nodes and owns
`NextAction`/`GoBack` routing), `add-trader-agent` (consumes `consensus_summary`), `add-risk-management`
(follows the same cyclic debate pattern).

## Goals / Non-Goals

- **Goals:**
    - Implement three `rig` agents (Bullish Researcher, Bearish Researcher, Debate Moderator) each with a
      domain-specific system prompt derived from `docs/prompts.md` and plain-text output suitable for
      `DebateMessage.content`.
    - Use the `DeepThinking` tier from `llm-providers` for all researcher agents.
    - Provide a `run_researcher_debate` function that executes the cyclic debate loop: for each round,
      invoke Bullish then Bearish sequentially, append their arguments to `TradingState::debate_history`,
      and after the final round invoke the Debate Moderator to produce `TradingState::consensus_summary`.
    - The round runner MUST respect `Config.llm.max_debate_rounds` (default 3) and terminate deterministically.
    - Each agent invocation records `AgentTokenUsage` with per-round granularity so the upstream orchestrator
      can build per-round `PhaseTokenUsage` entries.
    - Maintain chat history across rounds so each researcher can directly address the counterpart's prior arguments.
    - Confine all implementation to `src/agents/researcher/` without modifying foundation, provider, or other
      agent-owned files, while still allowing private helper modules inside `src/agents/researcher/` when needed to
      reduce duplication.
    - Use the provider layer's retry-wrapped chat path in a way that preserves authoritative usage metadata for
      researcher debate rounds, even if that requires the approved minimal cross-owner addition in
      `src/providers/factory.rs`.

- **Non-Goals:**
    - Implementing the `graph_flow::Task` wrapper or `NextAction` routing — belongs to `add-graph-orchestration`.
    - Implementing the risk debate loop — belongs to `add-risk-management` (though it follows a similar pattern).
    - Modifying `DebateMessage` or `TradingState` — the existing `core-types` definition is sufficient.
    - Tool bindings — researchers are reasoning agents that interpret analyst data, not tool-calling agents.
    - Per-agent provider overrides — the MVP uses tier-level provider config only.

## Cross-Owner Dependencies

The researcher module is intended to remain self-contained, but one approved cross-owner touch-point is required to
fully satisfy token-accounting requirements for chat-based debate rounds:

- `src/providers/factory.rs` (owner: `add-llm-providers`) may receive a minimal addition such as
  `chat_with_retry_details` and a supporting provider-agnostic chat-details surface on `LlmAgent` so Bullish and
  Bearish researchers can capture provider-reported usage metadata without duplicating provider-specific chat logic.

No other cross-owner changes are currently required. Existing foundation-owned contracts already cover this capability:
`max_debate_rounds` exists in `src/config.rs`, `DebateMessage`/`debate_history`/`consensus_summary` exist in
`src/state/trading_state.rs`, and unavailable-token metadata is already represented in `src/state/token_usage.rs`.

## Architectural Overview

```
src/agents/researcher/
├── mod.rs           <- Re-exports + run_researcher_debate cyclic loop function
├── bullish.rs       <- Bullish Researcher agent
├── bearish.rs       <- Bearish Researcher agent
└── moderator.rs     <- Debate Moderator agent
```

### Agent Construction Pattern

Each researcher follows a uniform construction pattern:

1. Obtain a `DeepThinking` completion model from the provider factory.
2. Build a `rig` agent via the agent builder helper with a system prompt sourced from constants matching
   `docs/prompts.md`. No tool bindings are attached; researchers are pure reasoning agents.
3. Serialize analyst outputs from `TradingState` (fundamental, technical, sentiment, news data) into prompt context.
4. Use the provider layer's retry-wrapped chat helper to maintain conversation history across rounds. To satisfy token
   accounting, the researcher layer expects a chat-details path that returns both response text and provider usage
   metadata.
5. Validate the returned plain-text response before storing it in `DebateMessage.content`, rejecting control characters
   or oversized payloads with `TradingError::SchemaViolation` using the same bounded-summary pattern established by the
   analyst layer.
6. Record `AgentTokenUsage` (model ID, token counts when available, `token_counts_available`, wall-clock latency).
7. Return `Result<(DebateMessage, AgentTokenUsage), TradingError>`.

### Cyclic Debate Loop

```
run_researcher_debate(state, config, providers)
  │
  │  for round in 1..=max_debate_rounds:
  │    │
  │    ├─ Bullish Researcher (chat_with_retry)
  │    │   ├─ Input: analyst data + debate_history + bear's latest argument
  │    │   ├─ Output: DebateMessage { role: "bullish_researcher", content }
  │    │   └─ Append to TradingState::debate_history
  │    │
  │    └─ Bearish Researcher (chat_with_retry)
  │        ├─ Input: analyst data + debate_history + bull's latest argument
  │        ├─ Output: DebateMessage { role: "bearish_researcher", content }
  │        └─ Append to TradingState::debate_history
  │
  │  After all rounds:
  │    └─ Debate Moderator (prompt_with_retry)
  │        ├─ Input: analyst data + full debate_history
  │        ├─ Output: plain-text consensus_summary
  │        └─ Write to TradingState::consensus_summary
  │
  └─ Return Vec<AgentTokenUsage> for all invocations
```

### Context Injection Strategy

Researchers receive serialized analyst data snapshots in their system prompt or as initial context:

| Placeholder                                           | Source                                               |
|-------------------------------------------------------|------------------------------------------------------|
| `{fundamental_report}`                                | `serde_json::to_string(&state.fundamental_metrics)`  |
| `{technical_report}`                                  | `serde_json::to_string(&state.technical_indicators)` |
| `{sentiment_report}`                                  | `serde_json::to_string(&state.market_sentiment)`     |
| `{news_report}`                                       | `serde_json::to_string(&state.macro_news)`           |
| `{debate_history}`                                    | Formatted from `state.debate_history`                |
| `{current_bull_argument}` / `{current_bear_argument}` | Latest `DebateMessage` from counterpart              |
| `{past_memory_str}`                                   | Empty string for MVP (memory system deferred)        |
| `{ticker}`                                            | `state.asset_symbol`                                 |
| `{current_date}`                                      | `state.target_date`                                  |

Missing analyst outputs (from graceful degradation) are serialized as `"null"` — the researcher prompts
explicitly handle missing data per `docs/prompts.md`.

### Output Validation And Bounded State Writes

Researcher and moderator outputs remain plain text, but they still need the same bounded-state protections applied to
other LLM-authored summaries in the codebase:

- Debate messages and the final `consensus_summary` should be rejected if they contain disallowed control characters.
- Debate messages and the final `consensus_summary` should be length-bounded before being written into
  `TradingState`, returning `TradingError::SchemaViolation` on overflow.
- If implementation benefit justifies it, the researcher module may add a private `common.rs` helper under
  `src/agents/researcher/` mirroring the analyst team's local validation/token-accounting helpers without expanding the
  public module surface.

### Chat History Management

The `rig` chat history is maintained per researcher across rounds to enable direct argument/counter-argument
exchange. Each round appends to the same chat session:

- **Bullish Researcher** maintains a chat history where each round adds the bear's latest argument as a "user"
  message and receives the bull's response.
- **Bearish Researcher** maintains a separate chat history where each round adds the bull's latest argument as a
  "user" message and receives the bear's response.
- **Debate Moderator** uses a single one-shot prompt (not chat) since it evaluates the complete debate history at
  once after all rounds complete.

### Token Accounting

Each agent invocation (2 per round for Bullish + Bearish, plus 1 for the Moderator) records `AgentTokenUsage`.
`AgentTokenUsage.token_counts_available` must distinguish provider-reported counts from unavailable metadata so the
researcher layer follows the current `core-types` contract rather than assuming zero-valued counts are authoritative.
Because Bullish and Bearish researchers use history-aware chat rather than one-shot prompts, this capability depends on
an approved provider-layer addition that exposes retry-wrapped chat usage details from `src/providers/factory.rs`
instead of plain text alone.
The `run_researcher_debate` function returns the full `Vec<AgentTokenUsage>` grouped by invocation order so the
upstream orchestrator can create per-round `PhaseTokenUsage` entries (e.g., "Researcher Debate Round 1",
"Researcher Debate Round 2", "Researcher Debate Moderation").

## Key Decisions

- **Sequential within rounds, not parallel**: Bullish and Bearish researchers execute sequentially within each
  round because each must respond to the other's latest argument. This is the intended debate dynamic from the
  PRD — concurrent execution would prevent cross-examination.

- **Chat history via rig, not prompt stuffing**: Using `rig`'s chat message history rather than concatenating
  all prior turns into the system prompt. This preserves role boundaries, avoids prompt length explosion over
  multiple rounds, and aligns with how `llm-providers` exposes `chat_with_retry`. A minimal cross-owner provider
  addition is acceptable here so the same chat path can also return usage metadata.

- **Moderator is one-shot, not chat**: The Debate Moderator evaluates the complete debate after all rounds finish.
  It does not participate in each round of the local round runner and executes once at the end. The workflow-level
  cyclic edge remains owned by `add-graph-orchestration`. Within this capability it uses `prompt_with_retry` (not
  chat) because it has no prior conversation to continue.

- **Plain-text output, not structured JSON**: Per `docs/prompts.md`, researchers and the moderator produce
  plain-text outputs stored as `DebateMessage.content` and `consensus_summary: String` respectively. This
  matches the current `TradingState` schema without requiring new types.

- **No tool bindings**: Researchers interpret analyst data injected via prompt context. They do not call external
  APIs or computation tools. This is consistent with the PRD — only the analyst layer has tool bindings.

- **System prompts as module constants**: Same pattern as `add-analyst-team` — prompts embedded as `const &str`
  values, compile-time checked, version-controlled alongside agent code.

- **Bound plain-text outputs before state writes**: Even though researchers do not emit JSON schemas, their outputs are
  still untrusted LLM content. Length and control-character validation protects `TradingState` snapshots, CLI rendering,
  and downstream prompts from pathological payloads.

## Risks / Trade-offs

- **Prompt length growth**: Each round adds ~2 messages to chat history. With `max_debate_rounds = 3`, the final
  Bearish invocation sees 5 prior messages. Deep-thinking models handle this well, but cost/latency increases
  linearly with rounds. Mitigation: `max_debate_rounds` is configurable; default of 3 balances quality vs. cost.

- **Sequential latency**: The debate phase is inherently sequential (Bull -> Bear -> Bull -> Bear -> ... ->
  Moderator). With 3 rounds, that's 7 sequential LLM calls on the deep-thinking tier. Mitigation: this matches
  the original Python implementation's design; the adversarial quality requires sequential turns.

- **Missing analyst data**: If Phase 1 degraded gracefully (1 analyst failed), the researchers receive `null` for
  that analyst's data. The prompts instruct researchers to acknowledge gaps rather than fabricate data. Mitigation:
  integration tests verify debate quality doesn't silently degrade.

- **Token counting accuracy**: Same risk as analyst team — depends on provider exposing authoritative token metadata.
  Handled by the existing `core-types` unavailable-token representation.

## Open Questions

- Should the Debate Moderator have the ability to terminate the debate early (before `max_debate_rounds`) if
  consensus is reached? Recommendation: defer to future enhancement. The MVP uses a fixed round count for
  deterministic behavior, matching the original TradingAgents implementation. Early termination would require
  the moderator to evaluate after each round (doubling moderator invocations) with unclear benefit.
