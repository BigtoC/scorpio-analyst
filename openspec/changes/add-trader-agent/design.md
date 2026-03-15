# Design for `add-trader-agent`

## Context

The foundation layer (`core-types`, `config`, `error-handling`) and the provider layer (`llm-providers`) are both
specified and provide the types, configuration, and LLM invocation helpers this change depends on. The analyst team
(`add-analyst-team`) populates `TradingState` with four structured analyst outputs during Phase 1. The researcher
debate team (`add-researcher-debate`) synthesizes those outputs into a `consensus_summary` during Phase 2.

This change introduces Phase 3 — a single Trader Agent that reads the full pipeline state and produces a structured
`TradeProposal` JSON object for downstream risk evaluation. Unlike the researcher team (multi-turn chat, plain-text
output, cyclic loop), the Trader Agent is a one-shot structured-output agent that must return valid JSON conforming
to the `TradeProposal` schema defined in `src/state/proposal.rs`.

**Stakeholders:** `add-graph-orchestration` (wraps the trader into a sequential `graph_flow::Task` node),
`add-risk-management` (reads `trader_proposal` for risk evaluation), `add-fund-manager` (reads `trader_proposal`
for final approval/rejection).

## Goals / Non-Goals

- **Goals:**
    - Implement a single `TraderAgent` as a `rig` agent using the `DeepThinking` model tier with a system prompt
      derived from `docs/prompts.md` (Trader section).
    - Produce a structured `TradeProposal` JSON object via the `prompt_with_retry` one-shot invocation path from
      `llm-providers`.
    - Validate the LLM's JSON response against the `TradeProposal` schema before writing to
      `TradingState::trader_proposal`, rejecting malformed output with `TradingError::SchemaViolation`.
    - Record `AgentTokenUsage` for the single invocation (agent name "Trader Agent", model ID,
      prompt/completion/total tokens, wall-clock latency, `token_counts_available` flag).
    - Provide a `run_trader` function that accepts `&mut TradingState`, `&Config`, and provider references, writes
      the validated proposal, and returns `Result<AgentTokenUsage, TradingError>`.
    - Confine all implementation to `src/agents/trader.rs` (plus the approved `pub mod trader;` uncomment in
      `src/agents/mod.rs`).

- **Non-Goals:**
    - Implementing the `graph_flow::Task` wrapper — belongs to `add-graph-orchestration`.
    - Implementing risk evaluation of the proposal — belongs to `add-risk-management`.
    - Implementing approval/rejection — belongs to `add-fund-manager`.
    - Tool bindings — the Trader is a pure reasoning agent that interprets state injected via prompt context.
    - Chat/multi-turn interaction — the Trader produces a single response per invocation.
    - Per-agent provider overrides — the MVP uses tier-level provider config only.
    - Position sizing, take-profit ladders, or entry windows — not part of the `TradeProposal` schema.

## Cross-Owner Dependencies

One approved cross-owner touch-point is required:

- `src/agents/mod.rs` (owner: `add-project-foundation`) — uncomment the pre-declared `pub mod trader;` line.
  This is a single-line edit the foundation skeleton anticipated. No other foundation-owned files are modified.

No provider-layer cross-owner changes are required. The existing `prompt_with_retry` helper from `add-llm-providers`
already supports one-shot structured output extraction with usage metadata, which is exactly what the Trader needs.

## Architectural Overview

```
src/agents/
├── mod.rs           <- Uncomment `pub mod trader;` (cross-owner, foundation-owned)
└── trader.rs        <- TraderAgent struct + run_trader function (this change)
```

### Agent Construction Pattern

1. Obtain a `DeepThinking` completion model from the provider factory.
2. Build a `rig` agent via the agent builder helper with the Trader system prompt sourced from a module constant
   matching `docs/prompts.md`. No tool bindings are attached — the Trader is a pure reasoning agent.
3. Serialize the full `TradingState` context into prompt placeholders (analyst outputs, consensus summary).
4. Use `prompt_with_retry` (one-shot) to invoke the LLM and extract the JSON response.
5. Parse and validate the JSON response against the `TradeProposal` schema.
6. Write the validated `TradeProposal` to `TradingState::trader_proposal`.
7. Record `AgentTokenUsage` and return it.

### Context Injection Strategy

The Trader Agent receives the full pipeline state as prompt context:

| Placeholder            | Source                                               |
|------------------------|------------------------------------------------------|
| `{ticker}`             | `state.asset_symbol`                                 |
| `{current_date}`       | `state.target_date`                                  |
| `{consensus_summary}`  | `state.consensus_summary` (from Debate Moderator)    |
| `{fundamental_report}` | `serde_json::to_string(&state.fundamental_metrics)`  |
| `{technical_report}`   | `serde_json::to_string(&state.technical_indicators)` |
| `{sentiment_report}`   | `serde_json::to_string(&state.market_sentiment)`     |
| `{news_report}`        | `serde_json::to_string(&state.macro_news)`           |
| `{past_memory_str}`    | Empty string for MVP (memory system deferred)        |

Missing analyst outputs (from Phase 1 graceful degradation) are serialized as `"null"`. A missing
`consensus_summary` (if the debate phase failed or was skipped) is handled as an explicit absence in the prompt.

### Output Validation

The `TradeProposal` returned by the LLM is validated before being stored:

1. **JSON parse**: The raw LLM response must parse as valid JSON matching the `TradeProposal` struct via `serde_json`.
2. **Action validity**: `action` must deserialize to a valid `TradeAction` variant (`Buy`, `Sell`, `Hold`).
3. **Numeric validity**: `target_price` and `stop_loss` must be finite and positive (`> 0.0`). `confidence` must
   be finite (no NaN/Infinity).
4. **Rationale bounds**: `rationale` must be non-empty and must not exceed the module's documented length bound.
   Control characters are rejected.
5. **Failure mode**: Any validation failure returns `TradingError::SchemaViolation` with a descriptive message.
   The `prompt_with_retry` wrapper handles LLM-level retries before the validation layer is reached.

### Token Accounting

The single `prompt_with_retry` call records one `AgentTokenUsage` entry with:
- `agent_name`: `"Trader Agent"`
- `model_id`: the model ID from the provider factory
- Token counts: authoritative when the provider reports them, unavailable-marker otherwise
- `latency_ms`: wall-clock time from prompt submission to response receipt

The `run_trader` function returns `Result<AgentTokenUsage, TradingError>` so the upstream orchestrator can
incorporate it into a "Trader Synthesis" `PhaseTokenUsage` entry.

## Key Decisions

- **One-shot prompt, not chat**: The Trader synthesizes all available data in a single pass. There is no
  adversarial counterpart or iterative refinement. This matches the PRD and the original TradingAgents
  implementation where the Trader is a sequential one-pass node.

- **Structured JSON output via `prompt_with_retry`**: Unlike the researchers (plain text), the Trader must return
  a machine-parseable `TradeProposal`. Using `prompt_with_retry` with `serde_json` deserialization ensures the
  response conforms to the schema or is retried/rejected. This follows the same pattern used by the analyst team
  for structured output extraction.

- **No tool bindings**: The Trader interprets analyst data and debate consensus already present in the context.
  It does not need to fetch additional data or perform calculations. The PRD confirms that only the analyst layer
  has tool bindings.

- **System prompt as module constant**: Same pattern as `add-analyst-team` and `add-researcher-debate` — the
  prompt is embedded as a `const &str` value, compile-time checked, version-controlled alongside agent code.

- **Validate numeric fields post-parse**: `serde_json` will accept any finite f64, but the domain requires
  `target_price > 0`, `stop_loss > 0`, and `confidence` to be finite. Post-parse validation catches
  semantically invalid values that pass JSON syntax checks.

- **Require consensus_summary but handle its absence**: The Trader's primary input is the debate consensus.
  If `consensus_summary` is `None` (e.g., debate phase was skipped in a test or degraded scenario), the prompt
  explicitly notes the absence rather than fabricating a consensus. The agent can still produce a proposal from
  analyst data alone, though the quality is expected to be lower.

## Risks / Trade-offs

- **Schema compliance dependence on LLM**: The deep-thinking model must return valid JSON matching `TradeProposal`.
  If it returns malformed JSON or extra fields, `serde_json` deserialization fails. Mitigation: `prompt_with_retry`
  retries up to 3 times with exponential backoff; the system prompt explicitly constrains the output format; and
  `TradingError::SchemaViolation` provides a clear failure path.

- **Missing consensus_summary**: If the debate phase fails, the Trader operates on analyst data alone without
  adversarial cross-examination. Mitigation: the prompt acknowledges the gap; the downstream Risk Management Team
  provides an additional validation layer. In practice, debate failure should be rare since it propagates a
  `TradingError` that aborts the cycle.

- **f64 precision in JSON**: JSON serialization of f64 can introduce minor floating-point representation changes.
  Mitigation: the `TradeProposal` fields use f64 consistently, and the validation step checks for finiteness rather
  than exact equality.

- **Token counting accuracy**: Same risk as all other agents — depends on the provider exposing authoritative token
  metadata. Handled by the existing `core-types` unavailable-token representation
  (`token_counts_available = false`).

## Open Questions

- None currently. The Trader Agent is well-defined in the PRD, architect plan, and prompt specification.
  The single-agent one-shot pattern is straightforward compared to the cyclic debate loops.
