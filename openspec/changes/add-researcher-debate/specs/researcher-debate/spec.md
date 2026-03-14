# `researcher-debate` Capability

## ADDED Requirements

### Requirement: Bullish Researcher Agent

The system MUST implement a Bullish Researcher agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Bull Researcher section), incorporating the target asset symbol
  and current date at construction time.
- No tool bindings — the Bullish Researcher is a pure reasoning agent that interprets analyst data injected via
  prompt context.

The agent MUST receive serialized analyst outputs (`FundamentalData`, `TechnicalData`, `SentimentData`, `NewsData`)
from `TradingState` as prompt context, along with the current debate history and the Bearish Researcher's latest
argument. The agent MUST produce a plain-text bullish argument suitable for storage as `DebateMessage.content` with
`role = "bullish_researcher"`. The agent MUST use the history-aware retry-wrapped chat path from `llm-providers` to
maintain conversation history across debate rounds, enabling direct counter-argument exchange. That provider path MUST
also expose usage metadata needed for `AgentTokenUsage` recording. The agent MUST record `AgentTokenUsage` (agent name
"Bullish Researcher", model ID, prompt/completion/total tokens, wall-clock latency).
Before writing the output into `TradingState::debate_history`, the agent MUST reject disallowed control characters and
lengths that exceed the module's documented bounded-summary policy by returning `TradingError::SchemaViolation`.

#### Scenario: First Round Bullish Argument

- **WHEN** the Bullish Researcher is invoked for the first debate round with populated analyst data and no prior
  debate history
- **THEN** the agent produces a plain-text argument synthesizing the bullish case from the analyst data, stored as
  `DebateMessage { role: "bullish_researcher", content }` and appended to `TradingState::debate_history`

#### Scenario: Subsequent Round Counter-Argument

- **WHEN** the Bullish Researcher is invoked for round 2+ with the Bearish Researcher's latest argument in the
  debate history
- **THEN** the agent directly addresses the bear's specific claims rather than repeating a generic bull thesis,
  building on the accumulated chat history

#### Scenario: Partial Analyst Data Available

- **WHEN** the Bullish Researcher is invoked and one analyst output is `None` (from Phase 1 graceful degradation)
- **THEN** the agent acknowledges the missing data gap in its argument rather than fabricating supporting evidence

### Requirement: Bearish Researcher Agent

The system MUST implement a Bearish Researcher agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Bear Researcher section), incorporating the target asset symbol
  and current date at construction time.
- No tool bindings — the Bearish Researcher is a pure reasoning agent that interprets analyst data injected via
  prompt context.

The agent MUST receive serialized analyst outputs from `TradingState` as prompt context, along with the current
debate history and the Bullish Researcher's latest argument. The agent MUST produce a plain-text bearish
counter-argument suitable for storage as `DebateMessage.content` with `role = "bearish_researcher"`. The agent MUST
use the history-aware retry-wrapped chat path from `llm-providers` to maintain conversation history across debate
rounds. That provider path MUST also expose usage metadata needed for `AgentTokenUsage` recording. The agent MUST
record `AgentTokenUsage` (agent name "Bearish Researcher", model ID, prompt/completion/total tokens, wall-clock
latency).
Before writing the output into `TradingState::debate_history`, the agent MUST reject disallowed control characters and
lengths that exceed the module's documented bounded-summary policy by returning `TradingError::SchemaViolation`.

#### Scenario: First Round Bearish Argument

- **WHEN** the Bearish Researcher is invoked for the first debate round with populated analyst data and the
  Bullish Researcher's opening argument
- **THEN** the agent produces a plain-text counter-argument directly addressing the bull's claims and highlighting
  counter-indicators (insider selling, overextended valuations, macroeconomic headwinds, technical resistance),
  stored as `DebateMessage { role: "bearish_researcher", content }`

#### Scenario: Subsequent Round Rebuttal

- **WHEN** the Bearish Researcher is invoked for round 2+ with the Bullish Researcher's latest rebuttal in the
  debate history
- **THEN** the agent directly dismantles the bull's specific claims rather than repeating a generic bearish thesis

#### Scenario: Partial Analyst Data Available

- **WHEN** the Bearish Researcher is invoked and one analyst output is `None`
- **THEN** the agent acknowledges the missing data gap rather than fabricating a negative signal from absent data

### Requirement: Debate Moderator Agent

The system MUST implement a Debate Moderator agent as a `rig` agent using the `DeepThinking` model tier. The agent
MUST be constructed via the agent builder helper from `llm-providers` with:

- A system prompt derived from `docs/prompts.md` (Debate Moderator section), incorporating the target asset symbol
  and current date at construction time.
- No tool bindings.

The Debate Moderator MUST be invoked once after all debate rounds complete. It MUST receive the full debate history,
serialized analyst data, and the final bullish and bearish positions. The agent MUST evaluate evidence quality (not
tone), select the prevailing perspective, and produce a plain-text consensus summary that:

1. States an explicit stance using the words `Buy`, `Sell`, or `Hold`.
2. Includes the strongest bullish evidence, the strongest bearish evidence, and the most important unresolved
   uncertainty.
3. Is compact enough for direct storage in `TradingState::consensus_summary`.

The Debate Moderator MUST use the `prompt_with_retry` invocation path (one-shot, not chat) since it evaluates the
complete debate at once. The agent MUST record `AgentTokenUsage` (agent name "Debate Moderator", model ID,
prompt/completion/total tokens, wall-clock latency).
Before writing the output into `TradingState::consensus_summary`, the moderator MUST reject disallowed control
characters and lengths that exceed the module's documented bounded-summary policy by returning
`TradingError::SchemaViolation`.

#### Scenario: Moderator Produces Consensus After Full Debate

- **WHEN** the Debate Moderator is invoked after 3 rounds of debate with 6 `DebateMessage` entries in
  `debate_history`
- **THEN** the agent writes a plain-text consensus summary to `TradingState::consensus_summary` containing an
  explicit `Buy`, `Sell`, or `Hold` stance, key evidence from both sides, and the primary unresolved uncertainty

#### Scenario: Moderator Operates With Minimal Debate

- **WHEN** the Debate Moderator is invoked after 1 round of debate (2 `DebateMessage` entries)
- **THEN** the agent still produces a valid consensus summary reflecting the limited debate depth

#### Scenario: Moderator Operates With No Debate Rounds

- **WHEN** `max_debate_rounds` is configured to 0 and the Debate Moderator is invoked with an empty
  `debate_history`
- **THEN** the agent produces a consensus summary based solely on the analyst data, noting the absence of
  adversarial debate

### Requirement: Cyclic Debate Loop Orchestration

The system MUST provide a `run_researcher_debate` function that orchestrates the multi-round cyclic debate between
the Bullish Researcher, Bearish Researcher, and Debate Moderator. The function MUST:

1. Accept a mutable reference to `TradingState`, a reference to `Config`, and provider factory references.
2. Execute `Config.llm.max_debate_rounds` iterations of the debate cycle (default 3). In each round:
   a. Invoke the Bullish Researcher with the current debate history and the bear's latest argument (if any).
   b. Append the bullish `DebateMessage` to `TradingState::debate_history`.
   c. Invoke the Bearish Researcher with the updated debate history and the bull's latest argument.
   d. Append the bearish `DebateMessage` to `TradingState::debate_history`.
3. After all rounds complete, invoke the Debate Moderator with the full `TradingState` exactly once.
4. Write the moderator's consensus summary to `TradingState::consensus_summary`.
5. Return `Result<Vec<AgentTokenUsage>, TradingError>` containing all token usage entries from all researcher
   invocations (2 per round) plus the moderator invocation.

The Bullish and Bearish researchers MUST execute sequentially within each round (not in parallel) because each must
respond to the other's latest argument. This sequential execution is fundamental to the adversarial debate dynamic.

If any researcher or moderator invocation fails (LLM error, timeout, schema violation), the debate MUST abort with the
corresponding `TradingError` — partial debate results are not usable for downstream synthesis.

#### Scenario: Three-Round Debate Completes Successfully

- **WHEN** `run_researcher_debate` is invoked with `max_debate_rounds = 3` and all LLM calls succeed
- **THEN** `TradingState::debate_history` contains 6 `DebateMessage` entries (alternating bullish/bearish),
  `TradingState::consensus_summary` is populated with the moderator's synthesis, and the function returns
  7 `AgentTokenUsage` entries (6 researcher + 1 moderator)

#### Scenario: Single-Round Debate

- **WHEN** `run_researcher_debate` is invoked with `max_debate_rounds = 1`
- **THEN** `TradingState::debate_history` contains 2 `DebateMessage` entries, `consensus_summary` is populated,
  and 3 `AgentTokenUsage` entries are returned

#### Scenario: Zero-Round Debate

- **WHEN** `run_researcher_debate` is invoked with `max_debate_rounds = 0`
- **THEN** no debate messages are added to `debate_history`, the Debate Moderator is still invoked to produce a
  consensus from analyst data alone, and 1 `AgentTokenUsage` entry is returned

#### Scenario: Researcher Failure Aborts Debate

- **WHEN** the Bearish Researcher fails with a `TradingError` during round 2 of a 3-round debate
- **THEN** `run_researcher_debate` returns the error immediately without invoking the Debate Moderator, and
  `consensus_summary` remains `None`

#### Scenario: Debate Round Count Is Configurable

- **WHEN** `Config.llm.max_debate_rounds` is set to 5
- **THEN** the debate loop executes exactly 5 rounds before invoking the Debate Moderator

### Requirement: Researcher Token Usage Recording

Each researcher agent invocation MUST record an `AgentTokenUsage` entry immediately after the LLM completion call
returns. The entry MUST contain the agent's display name ("Bullish Researcher", "Bearish Researcher", or "Debate
Moderator"), the model ID used for the completion, and wall-clock latency measured from prompt/chat submission to
response receipt. When the provider exposes authoritative prompt/completion/total token counts, those MUST be
recorded. When the provider does not expose authoritative counts, the agent MUST preserve the documented
unavailable-token representation from `core-types`, including correctly setting `AgentTokenUsage.token_counts_available`
to `false`.

For Bullish and Bearish researcher chat turns, the provider layer MUST expose a retry-wrapped history-aware chat path
that returns both response content and usage metadata, so the researcher module does not need to reimplement
provider-specific chat handling to satisfy token accounting.

The `run_researcher_debate` function MUST return token usage entries with per-invocation granularity (not aggregated
per round) so the upstream `add-graph-orchestration` change can construct per-round `PhaseTokenUsage` entries
(e.g., "Researcher Debate Round 1", "Researcher Debate Round 2", "Researcher Debate Moderation").

#### Scenario: Token Usage Recorded Per Invocation

- **WHEN** a 3-round debate completes with all LLM calls succeeding on an OpenAI deep-thinking model
- **THEN** 7 `AgentTokenUsage` entries are returned, each containing agent name, model ID, accurate token counts,
  and measured wall-clock latency, ordered by invocation sequence

#### Scenario: Token Usage Recorded When Counts Unavailable

- **WHEN** researchers use a provider that does not report authoritative token counts (e.g., Copilot via ACP)
- **THEN** each `AgentTokenUsage` entry still contains agent name, model ID, and wall-clock latency, with token
  count fields using the documented unavailable representation

### Requirement: Researcher Module Boundary

This capability's implementation MUST remain limited to researcher agent concerns within
`src/agents/researcher/mod.rs`, `src/agents/researcher/bullish.rs`, `src/agents/researcher/bearish.rs`, and
`src/agents/researcher/moderator.rs`. It MAY add private helper modules under `src/agents/researcher/` when needed for
shared prompt formatting, output validation, or token-accounting logic, provided those helpers are not re-exported as
public API. It MUST re-export the `run_researcher_debate` function and individual
researcher types from `src/agents/researcher/mod.rs` for consumption by the downstream `add-graph-orchestration`
change. The researcher module MUST NOT modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`,
`src/rate_limit.rs`), data-layer files (`src/data/*`), indicator files (`src/indicators/*`), or analyst-owned files
(`src/agents/analyst/*`). It MAY make the approved minimal cross-owner change in `src/providers/factory.rs` to obtain
retry-wrapped chat usage metadata required by researcher token accounting.

#### Scenario: Downstream Orchestrator Import Path

- **WHEN** the downstream `add-graph-orchestration` change imports the researcher debate team
- **THEN** it uses `use scorpio_analyst::agents::researcher::{run_researcher_debate, ...}` and receives the
  debate loop function and researcher types through the agent module path

#### Scenario: Only Approved Provider Cross-Owner Change Is Allowed

- **WHEN** the researcher debate module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`, `src/error.rs`,
  `src/rate_limit.rs`, the data-layer `src/data/*`, the indicator `src/indicators/*`, and the analyst-owned
  `src/agents/analyst/*` files all remain unmodified, and any provider-layer change is limited to the approved minimal
  touch-point in `src/providers/factory.rs`
