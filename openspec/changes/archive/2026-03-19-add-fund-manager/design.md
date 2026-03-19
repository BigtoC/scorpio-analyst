## Context

The Fund Manager is the terminal node (Phase 5) of the TradingAgents pipeline. It receives the
`TradeProposal` from Phase 3; the three `RiskReport` objects plus `risk_discussion_history` from
Phase 4; and the upstream analyst context already present on `TradingState`, then renders an
`ExecutionStatus` (Approved/Rejected). The design aligns with the existing agent patterns already
used in `src/agents/trader/mod.rs` and `src/agents/risk/mod.rs`, while keeping the capability
owned by this change limited to Fund Manager concerns behind a stable `fund_manager` module API.

## Goals / Non-Goals

- Goals:
  - Implement an LLM-powered Fund Manager with deterministic safety fallback
  - Maintain full compatibility with the existing `TradingState`, `ExecutionStatus`, and
    `AgentTokenUsage` types from `add-project-foundation`
  - Follow the existing agent implementation patterns (trait-based inference abstraction for
    testability and a public `run_fund_manager` entry point)

- Non-Goals:
  - Brokerage API dispatch (deferred to `add-graph-orchestration` or future work)
  - Multi-turn conversational approval flow (deferred to `add-tui` Phase 2)
  - Risk-adjusted position sizing (not part of current `ExecutionStatus` schema)

## Decisions

- **Deterministic rejection before LLM**: If both `conservative_risk_report.flags_violation` and
  `neutral_risk_report.flags_violation` are `true`, the Fund Manager bypasses the LLM entirely
  and returns `Decision::Rejected` with a hardcoded rationale. This matches the PRD's safety-net
  requirement and avoids spending tokens on a foregone conclusion.
  - *Alternative*: Always call the LLM and let it decide. Rejected because the PRD explicitly
    mandates the deterministic fallback, and skipping the LLM saves cost and latency.

- **Trait-based inference abstraction**: A `FundManagerInference` trait mirrors the `TraderInference`
  pattern, allowing unit tests to inject mock LLM responses without hitting real providers.

- **Package-style module facade**: The production implementation is organized under
  `src/agents/fund_manager/` with `mod.rs` as the intentional public surface and focused sibling
  modules for orchestration, prompt construction, validation, token usage, and tests. This keeps
  the external API stable while preventing another monolithic agent file.

- **LLM response extraction is an implementation detail**: The behavior requires a validated
  `ExecutionStatus`, but the spec does not require either typed extraction or raw JSON parsing.
  Implementation may use `rig`'s typed path or plain-text JSON parsing, as long as it preserves the
  required validation and retry behavior and does not require provider API changes.

- **Prompt hardening without API change**: The Fund Manager prompt remains the section 5 prompt from
  `docs/prompts.md`, but the canonical text now includes an untrusted-context notice and explicit
  instructions to acknowledge missing risk or analyst inputs in `rationale`.

- **`decided_at` field**: Populated by the runtime with the authoritative decision timestamp when
  available and injected into the prompt as `{current_date}`. If the runtime does not provide a more
  precise timestamp, it may fall back to the analysis date already stored on `TradingState`. If the
  LLM returns a different value, the runtime overwrites it with the authoritative value it chose.

- **No new cross-owner type changes**: The existing `TradingState`, `RiskReport`, `ExecutionStatus`,
  and provider factory APIs are already sufficient for this capability. The only expected
  cross-owner edit is uncommenting `pub mod fund_manager;` in `src/agents/mod.rs`, pending owner or
  maintainer approval.

## Risks / Trade-offs

- **LLM hallucination risk** -> Mitigated by the deterministic fallback for the most dangerous case
  (both Conservative and Neutral flag violation) and by strict schema validation on the LLM response.
- **Single point of failure** -> The Fund Manager is the only path to `ExecutionStatus`. If it
  errors, the entire pipeline fails. Mitigated by the standard retry policy (3 retries, exponential
  backoff) and the per-agent timeout.

## Open Questions

- None. The PRD and prompt specification are clear on the Fund Manager's behavior.
