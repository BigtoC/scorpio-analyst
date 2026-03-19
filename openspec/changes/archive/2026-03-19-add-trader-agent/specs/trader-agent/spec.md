# `trader-agent` Capability

## ADDED Requirements

### Requirement: Trader Agent

The system MUST implement a Trader Agent as a `rig` agent using the `DeepThinking` model tier. The agent MUST be
constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Trader section), incorporating the target asset symbol and
  current date at construction time.
- No tool bindings — the Trader Agent is a pure reasoning agent that interprets pipeline state injected via
  prompt context.

The agent MUST receive the full `TradingState` context as prompt input, including:

- Serialized analyst outputs (`FundamentalData`, `TechnicalData`, `SentimentData`, `NewsData`) — missing outputs
  from Phase 1 graceful degradation MUST be serialized as `"null"`.
- The `consensus_summary` from the Debate Moderator — if absent, the prompt MUST explicitly note the absence
  rather than substituting an empty string.
- The target asset symbol and current date.
- `past_memory_str` — empty string for MVP (memory system deferred).

Before injected state reaches the model, the trader prompt builder MUST:

- Treat analyst and consensus content as untrusted context rather than instructions.
- Sanitize prompt-bound symbol/date values to prompt-safe character sets.
- Redact secret-like substrings from prompt-bound context.
- Bound each injected context field to a fixed maximum size to limit prompt growth.

The agent MUST use the `prompt_typed_with_retry` invocation path (one-shot typed prompt, not chat) from
`llm-providers` to produce a structured `TradeProposal`. The Trader prompt MUST instruct the model to align with the
moderator's stance unless the analyst evidence clearly justifies a different conclusion. If the final proposal
diverges from the consensus stance, the `rationale` MUST explain why. The agent MUST record `AgentTokenUsage` (agent
name "Trader Agent", model ID, prompt/completion/total tokens, wall-clock latency, `token_counts_available` flag).

#### Scenario: Full Pipeline State Available

- **WHEN** the Trader Agent is invoked with all four analyst outputs populated and a `consensus_summary` from
  the Debate Moderator
- **THEN** the agent produces a valid `TradeProposal` JSON object with `action`, `target_price`, `stop_loss`,
  `confidence`, and `rationale` fields, and writes it to `TradingState::trader_proposal`

#### Scenario: Partial Analyst Data Available

- **WHEN** the Trader Agent is invoked and one analyst output is `None` (from Phase 1 graceful degradation)
- **THEN** the agent acknowledges the missing data in its reasoning and still produces a valid `TradeProposal`,
  noting the data gap in the `rationale` field

#### Scenario: Missing Consensus Summary

- **WHEN** the Trader Agent is invoked with `consensus_summary` as `None`
- **THEN** the agent produces a valid `TradeProposal` based on the available analyst data alone, and the prompt
  explicitly notes the absence of adversarial debate consensus

#### Scenario: Proposal Diverges From Consensus With Justification

- **WHEN** the moderator's `consensus_summary` recommends `Hold` but the analyst evidence strongly supports a
  different action
- **THEN** the Trader Agent MAY return `Buy` or `Sell`, but the `rationale` explicitly explains why the analyst
  evidence outweighed the consensus stance

### Requirement: Trade Proposal Schema Validation

The system MUST validate the LLM's JSON response against the `TradeProposal` schema before writing to
`TradingState::trader_proposal`. Validation MUST enforce:

1. The provider-layer typed response successfully deserializes into the `TradeProposal` struct.
2. `action` deserializes to a valid `TradeAction` variant (`Buy`, `Sell`, or `Hold`).
3. `target_price` MUST be finite and greater than zero.
4. `stop_loss` MUST be finite and greater than zero.
5. `confidence` MUST be finite (no NaN or Infinity).
6. `rationale` MUST be non-empty, MUST NOT contain disallowed control characters, and MUST NOT exceed the
   module's documented length bound.
7. If `action` is `Hold`, `target_price` and `stop_loss` MUST still be present as numeric monitoring levels rather
   than being omitted or nulled.

If any validation check fails, the agent MUST return `TradingError::SchemaViolation` with a descriptive error
message identifying the specific violation. Provider-layer structured-output decoding failures and trader-layer
post-parse schema/domain validation failures MUST be treated as non-retriable schema violations rather than retried
against the same prompt.

#### Scenario: Valid Trade Proposal Accepted

- **WHEN** the LLM returns a JSON response with `action = "Buy"`, `target_price = 185.50`,
  `stop_loss = 178.00`, `confidence = 0.82`, and a non-empty `rationale`
- **THEN** the response is deserialized into a `TradeProposal` with `TradeAction::Buy` and written to
  `TradingState::trader_proposal`

#### Scenario: Invalid Target Price Rejected

- **WHEN** the LLM returns a JSON response with `target_price = -10.0` or `target_price = NaN`
- **THEN** the agent returns `TradingError::SchemaViolation` and `TradingState::trader_proposal` remains `None`

#### Scenario: Invalid Stop Loss Rejected

- **WHEN** the LLM returns a JSON response with `stop_loss = 0.0` or `stop_loss = Infinity`
- **THEN** the agent returns `TradingError::SchemaViolation` and `TradingState::trader_proposal` remains `None`

#### Scenario: Non-Finite Confidence Rejected

- **WHEN** the LLM returns a JSON response with `confidence = NaN` or `confidence = Infinity`
- **THEN** the agent returns `TradingError::SchemaViolation` and `TradingState::trader_proposal` remains `None`

#### Scenario: Empty Rationale Rejected

- **WHEN** the LLM returns a JSON response with an empty `rationale` string
- **THEN** the agent returns `TradingError::SchemaViolation` and `TradingState::trader_proposal` remains `None`

#### Scenario: Malformed JSON Rejected

- **WHEN** the LLM returns a response that does not parse as valid `TradeProposal` JSON
- **THEN** the typed provider path returns `TradingError::SchemaViolation` immediately, and
  `TradingState::trader_proposal` remains `None`

#### Scenario: Post-Parse Schema Violation Is Not Retried

- **WHEN** the provider successfully returns a typed `TradeProposal` but trader-layer validation rejects it because a
  required domain constraint fails (for example `target_price <= 0.0`)
- **THEN** the trader returns `TradingError::SchemaViolation` without retrying the same prompt, and
  `TradingState::trader_proposal` remains `None`

#### Scenario: Oversized Rationale Rejected

- **WHEN** the LLM returns a `rationale` that exceeds the module's documented length bound or contains
  disallowed control characters
- **THEN** the agent returns `TradingError::SchemaViolation` and `TradingState::trader_proposal` remains `None`

#### Scenario: Hold Proposal Uses Monitoring Levels

- **WHEN** the Trader Agent returns `action = "Hold"`
- **THEN** the resulting `TradeProposal` still contains numeric `target_price` and `stop_loss` values, interpreted as
  confirmation and thesis-break monitoring levels rather than immediate execution levels

### Requirement: Trader Token Usage Recording

The Trader Agent invocation MUST record an `AgentTokenUsage` entry immediately after the LLM completion call
returns. The entry MUST contain the agent's display name ("Trader Agent"), the model ID used for the completion,
and wall-clock latency measured from prompt submission to response receipt. When the provider exposes authoritative
prompt/completion/total token counts, those MUST be recorded. When the provider does not expose authoritative
counts, the agent MUST preserve the documented unavailable-token representation from `core-types`, including
correctly setting `AgentTokenUsage.token_counts_available` to `false`.

The `run_trader` function MUST return `Result<AgentTokenUsage, TradingError>` so the upstream
`add-graph-orchestration` change can incorporate it into a "Trader Synthesis" `PhaseTokenUsage` entry.

#### Scenario: Token Usage Recorded With Authoritative Counts

- **WHEN** the Trader Agent completes successfully using a provider that reports authoritative token counts
  (e.g., OpenAI)
- **THEN** the returned `AgentTokenUsage` contains agent name "Trader Agent", the correct model ID,
  `token_counts_available = true`, accurate prompt/completion/total token counts, and measured wall-clock latency

#### Scenario: Token Usage Recorded When Counts Unavailable

- **WHEN** the Trader Agent completes successfully using a provider that does not report authoritative token
  counts (e.g., Copilot via ACP)
- **THEN** the returned `AgentTokenUsage` contains agent name "Trader Agent", the correct model ID,
  `token_counts_available = false`, and measured wall-clock latency, with token count fields using the documented
  unavailable representation

### Requirement: Trader Module Boundary

This capability's implementation MUST remain limited to trader agent concerns within `src/agents/trader/mod.rs`. It
MUST re-export the `run_trader` function and `TraderAgent` type from `src/agents/trader/mod.rs` for consumption by
the downstream `add-graph-orchestration` change via the agent module path
(`use scorpio_analyst::agents::trader::{run_trader, TraderAgent}`).

The trader module MUST NOT modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`,
`src/rate_limit.rs`), provider-owned files (`src/providers/*`), data-layer files (`src/data/*`), indicator files
(`src/indicators/*`), analyst-owned files (`src/agents/analyst/*`), or researcher-owned files
(`src/agents/researcher/*`). It MAY make the approved single-line cross-owner change in `src/agents/mod.rs` to
uncomment `pub mod trader;`.

#### Scenario: Downstream Orchestrator Import Path

- **WHEN** the downstream `add-graph-orchestration` change imports the trader agent
- **THEN** it uses `use scorpio_analyst::agents::trader::{run_trader, TraderAgent}` and receives the trader
  function and type through the agent module path

#### Scenario: Only Approved Cross-Owner Change Is Allowed

- **WHEN** the trader agent module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`,
  `src/rate_limit.rs`, the provider-owned `src/providers/*`, the data-layer `src/data/*`, the indicator
  `src/indicators/*`, the analyst-owned `src/agents/analyst/*`, and the researcher-owned
  `src/agents/researcher/*` files all remain unmodified, and the only cross-owner change is the approved
  `pub mod trader;` uncomment in `src/agents/mod.rs`
