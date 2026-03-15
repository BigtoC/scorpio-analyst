# `risk-management` Capability

## ADDED Requirements

### Requirement: Aggressive Risk Agent

The system MUST implement an Aggressive Risk Agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Aggressive Risk Analyst section), incorporating the target asset
  symbol and current date at construction time.
- No tool bindings — the Aggressive Risk Agent is a pure reasoning agent that interprets analyst data and the trade
  proposal injected via prompt context.

The agent MUST receive the serialized `TradeProposal`, analyst outputs (`FundamentalData`, `TechnicalData`,
`SentimentData`, `NewsData`) from `TradingState`, the current risk discussion history, and the Conservative and Neutral
risk agents' latest views as prompt context. The agent MUST produce a structured `RiskReport` JSON output with
`risk_level = Aggressive`, containing an `assessment` string, `recommended_adjustments` array, and `flags_violation`
boolean. The agent MUST validate that the returned `risk_level` matches `RiskLevel::Aggressive` before writing to
`TradingState`, rejecting mismatches with `TradingError::SchemaViolation`. The agent MUST record `AgentTokenUsage`
(agent name "Aggressive Risk Analyst", model ID, prompt/completion/total tokens, wall-clock latency).

The Aggressive Risk Agent MUST maintain multi-round chat history using `chat_with_retry_details`, locally deserialize
the returned raw JSON string into `RiskReport`, and reject malformed JSON with `TradingError::SchemaViolation`.
Prompt-bound `TradeProposal`, analyst data, and risk-history context MUST be treated as untrusted context, sanitized
before injection, redact secret-like substrings, and keep bounded risk-history context so later rounds do not grow
unbounded prompt size. Before the `RiskReport` is written to `TradingState`, the agent MUST reject disallowed control
characters and values that exceed the module's documented bounded-text policy in `assessment` and in every
`recommended_adjustments` entry.

#### Scenario: First Round Aggressive Assessment

- **WHEN** the Aggressive Risk Agent is invoked for the first discussion round with a populated `TradeProposal` and
  analyst data but no prior risk discussion history
- **THEN** the agent produces a `RiskReport` with `risk_level = Aggressive` advocating for upside capture while
  identifying genuine risk controls, written to `TradingState::aggressive_risk_report`

#### Scenario: Subsequent Round With Other Agents' Views

- **WHEN** the Aggressive Risk Agent is invoked for round 2+ with the Conservative and Neutral agents' latest
  `RiskReport` outputs available in the risk discussion history
- **THEN** the agent directly addresses the main objections raised by the other risk analysts rather than repeating a
  generic aggressive stance

#### Scenario: Partial Analyst Data Available

- **WHEN** the Aggressive Risk Agent is invoked and one analyst output is `None` (from Phase 1 graceful degradation)
- **THEN** the agent acknowledges the missing data gap in its assessment rather than fabricating supporting evidence

#### Scenario: Risk Level Mismatch Rejected

- **WHEN** the LLM returns a `RiskReport` with `risk_level` not equal to `Aggressive`
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

#### Scenario: Oversized Assessment Or Adjustment Rejected

- **WHEN** the LLM returns an Aggressive `RiskReport` whose `assessment` or any `recommended_adjustments` entry
  contains disallowed control characters or exceeds the module's documented bounded-text policy
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

### Requirement: Conservative Risk Agent

The system MUST implement a Conservative Risk Agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Conservative Risk Analyst section), incorporating the target asset
  symbol and current date at construction time.
- No tool bindings.

The agent MUST receive the serialized `TradeProposal`, analyst outputs from `TradingState`, the current risk discussion
history, and the Aggressive and Neutral risk agents' latest views as prompt context. The agent MUST produce a structured
`RiskReport` JSON output with `risk_level = Conservative`, evaluating the proposal entirely from the perspective of
Maximum Drawdown and capital preservation. The agent MUST actively evaluate trades for overbought RSI conditions, severe
macroeconomic uncertainty, or high beta relative to the broader market. The agent MUST validate that the returned
`risk_level` matches `RiskLevel::Conservative` before writing to `TradingState`, rejecting mismatches with
`TradingError::SchemaViolation`. The agent MUST set `flags_violation` to `true` when the proposal has a material
risk-control flaw or unjustified exposure. The agent MUST record `AgentTokenUsage` (agent name "Conservative Risk
Analyst", model ID, prompt/completion/total tokens, wall-clock latency).

The Conservative Risk Agent MUST maintain multi-round chat history using `chat_with_retry_details`, locally deserialize
the returned raw JSON string into `RiskReport`, and reject malformed JSON with `TradingError::SchemaViolation`.
Prompt-bound `TradeProposal`, analyst data, and risk-history context MUST be treated as untrusted context, sanitized
before injection, redact secret-like substrings, and keep bounded risk-history context so later rounds do not grow
unbounded prompt size. Before the `RiskReport` is written to `TradingState`, the agent MUST reject disallowed control
characters and values that exceed the module's documented bounded-text policy in `assessment` and in every
`recommended_adjustments` entry.

#### Scenario: First Round Conservative Assessment

- **WHEN** the Conservative Risk Agent is invoked for the first discussion round with a populated `TradeProposal`
- **THEN** the agent produces a `RiskReport` with `risk_level = Conservative` focused on capital preservation,
  downside risk, and control adequacy, written to `TradingState::conservative_risk_report`

#### Scenario: Conservative Flags Violation

- **WHEN** the `TradeProposal` exhibits overbought RSI conditions or severe macroeconomic uncertainty per the
  analyst data
- **THEN** the Conservative Risk Agent sets `flags_violation = true` in its `RiskReport` with a specific
  justification in the `assessment` field

#### Scenario: Risk Level Mismatch Rejected

- **WHEN** the LLM returns a `RiskReport` with `risk_level` not equal to `Conservative`
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

#### Scenario: Oversized Assessment Or Adjustment Rejected

- **WHEN** the LLM returns a Conservative `RiskReport` whose `assessment` or any `recommended_adjustments` entry
  contains disallowed control characters or exceeds the module's documented bounded-text policy
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

### Requirement: Neutral Risk Agent

The system MUST implement a Neutral Risk Agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Neutral Risk Analyst section), incorporating the target asset
  symbol and current date at construction time.
- No tool bindings.

The agent MUST receive the serialized `TradeProposal`, analyst outputs from `TradingState`, the current risk discussion
history, and the Aggressive and Conservative risk agents' latest views as prompt context. The agent MUST produce a
structured `RiskReport` JSON output with `risk_level = Neutral`, functioning as the moderating force that attempts to
optimize the Sharpe Ratio by balancing aggressive upside targets against conservative downside protections. The agent
MUST validate that the returned `risk_level` matches `RiskLevel::Neutral` before writing to `TradingState`, rejecting
mismatches with `TradingError::SchemaViolation`. The agent MUST set `flags_violation` to `true` only when the proposal
fails even a balanced risk test. The agent MUST record `AgentTokenUsage` (agent name "Neutral Risk Analyst", model ID,
prompt/completion/total tokens, wall-clock latency).

The Neutral Risk Agent MUST maintain multi-round chat history using `chat_with_retry_details`, locally deserialize the
returned raw JSON string into `RiskReport`, and reject malformed JSON with `TradingError::SchemaViolation`.
Prompt-bound `TradeProposal`, analyst data, and risk-history context MUST be treated as untrusted context, sanitized
before injection, redact secret-like substrings, and keep bounded risk-history context so later rounds do not grow
unbounded prompt size. Before the `RiskReport` is written to `TradingState`, the agent MUST reject disallowed control
characters and values that exceed the module's documented bounded-text policy in `assessment` and in every
`recommended_adjustments` entry.

#### Scenario: First Round Neutral Assessment

- **WHEN** the Neutral Risk Agent is invoked for the first discussion round with a populated `TradeProposal`
- **THEN** the agent produces a `RiskReport` with `risk_level = Neutral` weighing upside and downside fairly,
  written to `TradingState::neutral_risk_report`

#### Scenario: Neutral Balances Extremes

- **WHEN** the Aggressive agent advocates wider stops and the Conservative agent demands strict capital preservation
- **THEN** the Neutral agent identifies where the Aggressive view is too permissive and where the Conservative view
  is too restrictive, proposing balanced refinements in `recommended_adjustments`

#### Scenario: Risk Level Mismatch Rejected

- **WHEN** the LLM returns a `RiskReport` with `risk_level` not equal to `Neutral`
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

#### Scenario: Oversized Assessment Or Adjustment Rejected

- **WHEN** the LLM returns a Neutral `RiskReport` whose `assessment` or any `recommended_adjustments` entry
  contains disallowed control characters or exceeds the module's documented bounded-text policy
- **THEN** the agent returns `TradingError::SchemaViolation` without writing the invalid report to `TradingState`

### Requirement: Risk Moderator Agent

The system MUST implement a Risk Moderator agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Risk Moderator section), incorporating the target asset symbol
  and current date at construction time.
- No tool bindings.

The Risk Moderator MUST be invoked once after all risk discussion rounds complete. It MUST receive the full risk
discussion history, the three latest `RiskReport` objects, the `TradeProposal`, and serialized analyst data. The agent
MUST synthesize the three risk perspectives into a concise plain-text discussion summary that:

1. Identifies the main agreement points and the true blockers.
2. Evaluates whether the trader's proposal is adequately defended on target, stop, and confidence.
3. Explicitly notes whether Conservative and Neutral both flag a material violation, because the Fund Manager uses that
   as a deterministic rejection rule.
4. Is compact enough for direct storage as a `DebateMessage.content` entry in
   `TradingState::risk_discussion_history`.

The Risk Moderator MUST use the `prompt_with_retry_details` invocation path (one-shot, not chat) since it evaluates the
complete discussion at once. The agent MUST record `AgentTokenUsage` (agent name "Risk Moderator", model ID,
prompt/completion/total tokens, wall-clock latency).
Before writing the output into `TradingState::risk_discussion_history`, the moderator MUST reject disallowed control
characters and lengths that exceed the module's documented bounded-summary policy by returning
`TradingError::SchemaViolation`.

The Risk Moderator MUST treat the injected trade proposal, analyst data, and risk reports as untrusted context,
sanitize prompt-bound symbol/date values, redact secret-like substrings, and bound risk-history context before LLM
injection.

#### Scenario: Moderator Produces Synthesis After Full Discussion

- **WHEN** the Risk Moderator is invoked after 2 rounds of discussion with 6 risk persona `DebateMessage` entries
  in `risk_discussion_history`
- **THEN** the agent appends a plain-text synthesis as `DebateMessage { role: "risk_moderator", content }` to
  `TradingState::risk_discussion_history`, noting agreement points, blockers, and the violation flag status

#### Scenario: Moderator Notes Dual Violation Flag

- **WHEN** both the Conservative and Neutral `RiskReport` objects have `flags_violation = true`
- **THEN** the Risk Moderator's synthesis explicitly states that both Conservative and Neutral flag a material
  violation, alerting the downstream Fund Manager to the deterministic rejection condition

#### Scenario: Moderator Operates With Single Round

- **WHEN** the Risk Moderator is invoked after 1 round of discussion (3 risk persona `DebateMessage` entries)
- **THEN** the agent still produces a valid synthesis reflecting the limited discussion depth

#### Scenario: Moderator Operates With No Discussion Rounds

- **WHEN** `max_risk_rounds` is configured to 0 and the Risk Moderator is invoked with an empty
  `risk_discussion_history` but populated `RiskReport` fields are `None`
- **THEN** the agent produces a synthesis based solely on the `TradeProposal` and analyst data, noting the absence
  of risk discussion

### Requirement: Cyclic Risk Discussion Loop Orchestration

The system MUST provide a `run_risk_discussion` function that orchestrates the multi-round cyclic discussion among
the Aggressive, Conservative, and Neutral risk agents plus the Risk Moderator. The function MUST:

1. Accept a mutable reference to `TradingState`, a reference to `Config`, and provider factory references.
2. Validate that `TradingState::trader_proposal` is `Some`, returning a `TradingError` if absent.
3. Execute `Config.llm.max_risk_rounds` iterations of the discussion cycle (default 2). In each round:
   a. Invoke the Aggressive Risk Agent with the current risk discussion history and other agents' latest views.
   b. Write the Aggressive `RiskReport` to `TradingState::aggressive_risk_report` and append a `DebateMessage`
      with `role = "aggressive_risk"` to `TradingState::risk_discussion_history`.
   c. Invoke the Conservative Risk Agent with the updated context.
   d. Write the Conservative `RiskReport` to `TradingState::conservative_risk_report` and append a `DebateMessage`
      with `role = "conservative_risk"` to `TradingState::risk_discussion_history`.
   e. Invoke the Neutral Risk Agent with the updated context.
   f. Write the Neutral `RiskReport` to `TradingState::neutral_risk_report` and append a `DebateMessage`
      with `role = "neutral_risk"` to `TradingState::risk_discussion_history`.
4. After all rounds complete, invoke the Risk Moderator with the full `TradingState` exactly once.
5. Append the moderator's synthesis to `TradingState::risk_discussion_history` as a `DebateMessage` with
   `role = "risk_moderator"`.
6. Return `Result<Vec<AgentTokenUsage>, TradingError>` containing all token usage entries from all risk agent
   invocations (3 per round) plus the moderator invocation.

The three risk persona agents MUST execute sequentially within each round (not in parallel) because each agent's prompt
references the other agents' latest views, and sequential execution enables later agents in a round to see earlier
agents' output from the same round. This sequential execution is fundamental to the progressive refinement of the risk
assessment. This requirement is the normative Phase 4 behavior for the current prompt contract even though the broader
project architecture informally describes the risk phase as "fan-out + cyclic debate."

The `run_risk_discussion` entry point MUST construct one shared `DeepThinking` completion model handle and pass it to
all three persona agents plus the moderator, matching the provider-sharing pattern already used by
`run_researcher_debate`.

If any risk agent or moderator invocation fails (LLM error, timeout, schema violation), the discussion MUST abort with
the corresponding `TradingError` — partial risk results are not usable for downstream Fund Manager review.

#### Scenario: Two-Round Discussion Completes Successfully

- **WHEN** `run_risk_discussion` is invoked with `max_risk_rounds = 2` and all LLM calls succeed
- **THEN** `TradingState::risk_discussion_history` contains 6 risk persona `DebateMessage` entries plus 1 moderator
  entry (7 total), all 3 `RiskReport` fields are populated with the final round's reports, and the function returns
  7 `AgentTokenUsage` entries (6 persona + 1 moderator)

#### Scenario: Single-Round Discussion

- **WHEN** `run_risk_discussion` is invoked with `max_risk_rounds = 1`
- **THEN** `TradingState::risk_discussion_history` contains 3 risk persona `DebateMessage` entries plus 1 moderator
  entry (4 total), all 3 `RiskReport` fields are populated, and 4 `AgentTokenUsage` entries are returned

#### Scenario: Zero-Round Discussion

- **WHEN** `run_risk_discussion` is invoked with `max_risk_rounds = 0`
- **THEN** no risk persona messages are added to `risk_discussion_history`, the Risk Moderator is still invoked to
  produce a synthesis from the trade proposal alone, and 1 `AgentTokenUsage` entry is returned

#### Scenario: Risk Agent Failure Aborts Discussion

- **WHEN** the Conservative Risk Agent fails with a `TradingError` during round 2 of a 2-round discussion
- **THEN** `run_risk_discussion` returns the error immediately without invoking remaining agents or the Risk
  Moderator

#### Scenario: Missing TradeProposal Prevents Discussion

- **WHEN** `run_risk_discussion` is invoked but `TradingState::trader_proposal` is `None`
- **THEN** the function returns a `TradingError` immediately without invoking any risk agents

#### Scenario: Discussion Round Count Is Configurable

- **WHEN** `Config.llm.max_risk_rounds` is set to 4
- **THEN** the discussion loop executes exactly 4 rounds before invoking the Risk Moderator

### Requirement: Risk Agent Token Usage Recording

Each risk agent invocation MUST record an `AgentTokenUsage` entry immediately after the LLM completion call returns.
The entry MUST contain the agent's display name ("Aggressive Risk Analyst", "Conservative Risk Analyst", "Neutral Risk
Analyst", or "Risk Moderator"), the model ID used for the completion, and wall-clock latency measured from prompt
submission to response receipt. When the provider exposes authoritative prompt/completion/total token counts, those MUST
be recorded. When the provider does not expose authoritative counts, the agent MUST preserve the documented
unavailable-token representation from `core-types`, including correctly setting `AgentTokenUsage.token_counts_available`
to `false`.

The `run_risk_discussion` function MUST return token usage entries with per-invocation granularity (not aggregated per
round) so the upstream `add-graph-orchestration` change can construct per-round `PhaseTokenUsage` entries (e.g.,
"Risk Discussion Round 1", "Risk Discussion Round 2", "Risk Discussion Moderation").

#### Scenario: Token Usage Recorded Per Invocation

- **WHEN** a 2-round risk discussion completes with all LLM calls succeeding on an OpenAI deep-thinking model
- **THEN** 7 `AgentTokenUsage` entries are returned, each containing agent name, model ID, accurate token counts,
  and measured wall-clock latency, ordered by invocation sequence

#### Scenario: Token Usage Recorded When Counts Unavailable

- **WHEN** risk agents use a provider that does not report authoritative token counts (e.g., Copilot via ACP)
- **THEN** each `AgentTokenUsage` entry still contains agent name, model ID, and wall-clock latency, with token
  count fields using the documented unavailable representation

### Requirement: Risk Module Boundary

This capability's implementation MUST remain limited to risk agent concerns within `src/agents/risk/mod.rs`,
`src/agents/risk/aggressive.rs`, `src/agents/risk/conservative.rs`, `src/agents/risk/neutral.rs`, and
`src/agents/risk/moderator.rs`. It MAY add private helper modules under `src/agents/risk/` when needed for shared
prompt formatting, output validation, or token-accounting logic, provided those helpers are not re-exported as public
API. It MUST re-export the `run_risk_discussion` function and individual risk agent types from
`src/agents/risk/mod.rs` for consumption by the downstream `add-graph-orchestration` change. The risk module MUST NOT
modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`, `src/rate_limit.rs`), provider-owned
files (`src/providers/*`), data-layer files (`src/data/*`), indicator files (`src/indicators/*`), analyst-owned files
(`src/agents/analyst/*`), researcher-owned files (`src/agents/researcher/*`), or trader-owned files
(`src/agents/trader/*`).

#### Scenario: Downstream Orchestrator Import Path

- **WHEN** the downstream `add-graph-orchestration` change imports the risk management team
- **THEN** it uses `use scorpio_analyst::agents::risk::{run_risk_discussion, ...}` and receives the discussion
  loop function and risk agent types through the agent module path

#### Scenario: No Cross-Owner File Modifications

- **WHEN** the risk management module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`,
  `src/rate_limit.rs`, the provider-owned `src/providers/*`, the data-layer `src/data/*`, the indicator
  `src/indicators/*`, the analyst-owned `src/agents/analyst/*`, the researcher-owned `src/agents/researcher/*`,
  and the trader-owned `src/agents/trader.rs` files all remain unmodified
