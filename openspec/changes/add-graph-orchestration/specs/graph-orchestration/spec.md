# `graph-orchestration` Capability

## ADDED Requirements

### Requirement: Graph-Flow Pipeline Construction

The system MUST construct a graph-flow directed graph with 11 task nodes wired via `GraphBuilder`. The graph topology
MUST follow a 5-phase structure:

**Phase 1 — Analyst Fan-Out:**
- `FanOutTask("analyst_fanout")` containing 4 child tasks: `FundamentalAnalystTask`, `SentimentAnalystTask`,
  `NewsAnalystTask`, `TechnicalAnalystTask`.
- The 4 analyst child tasks MUST execute in parallel, not sequentially, because the upstream analyst-team capability
  defines them as independent concurrent tasks that populate separate analyst-owned fields.
- Edge: `analyst_fanout` → `AnalystSyncTask`.

**Phase 2 — Researcher Debate:**
- `BullishResearcherTask` → `BearishResearcherTask` → `DebateModeratorTask`.
- Conditional edge from `DebateModeratorTask`: if `debate_round < max_debate_rounds`, loop back to
  `BullishResearcherTask`; otherwise continue to Phase 3.

**Phase 3 — Trader:**
- `TraderTask` (sequential, single node).

**Phase 4 — Risk Discussion:**
- `AggressiveRiskTask` → `ConservativeRiskTask` → `NeutralRiskTask` → `RiskModeratorTask`.
- Conditional edge from `RiskModeratorTask`: if `risk_round < max_risk_rounds`, loop back to
  `AggressiveRiskTask`; otherwise continue to Phase 5.

**Phase 5 — Fund Manager:**
- `FundManagerTask` → End.

**Entry Conditional Edges:**
- From `AnalystSyncTask`: if `max_debate_rounds > 0`, proceed to `BullishResearcherTask`; else proceed directly
  to `DebateModeratorTask` (skipping the researcher debate loop).
- From `TraderTask` (after phase snapshot): if `max_risk_rounds > 0`, proceed to `AggressiveRiskTask`; else
  proceed directly to `RiskModeratorTask` (skipping the risk persona tasks).

The pipeline MUST use `InMemorySessionStorage` for graph-flow session management. The pipeline MUST consume the forked
`graph-flow` dependency from `BigtoC/rs-graph-llm` branch `feature/update-rig-version`, with the `rig` feature enabled
against the project's `rig-core 0.32`.

The pipeline MUST emit structured `tracing` events for analysis-cycle start/end, phase transitions, debate/risk round
boundaries, task start/success/failure, and snapshot persistence so downstream interface layers can stream live
workflow progress.

#### Scenario: Graph Builds Successfully With All Nodes

- **WHEN** `GraphBuilder` is invoked with the full 5-phase topology
- **THEN** the resulting graph contains exactly 11 task nodes with all edges wired according to the phase structure,
  and the start task is set to `analyst_fanout`

#### Scenario: Pipeline Execution Proceeds Through All Phases

- **WHEN** the graph is executed via `FlowRunner` with a fully seeded `Context`
- **THEN** execution proceeds through Phase 1 (analyst fan-out), Phase 2 (researcher debate), Phase 3 (trader),
  Phase 4 (risk discussion), and Phase 5 (fund manager) in strict sequential order

#### Scenario: Start Task Is Analyst Fanout

- **WHEN** the pipeline is constructed
- **THEN** the graph's designated start task is `analyst_fanout`, ensuring execution begins with the analyst phase

### Requirement: Context-as-Envelope State Bridging

The system MUST serialize the full `TradingState` as a single JSON blob into graph-flow `Context` under the key
`"trading_state"`. Each task MUST deserialize `TradingState` from `Context` on entry, operate on the typed state, and
re-serialize back to `Context` on exit.

`Config.llm.max_debate_rounds` and `Config.llm.max_risk_rounds` MUST be stored in `Context` under keys
`"max_debate_rounds"` and `"max_risk_rounds"` at pipeline initialization, so that conditional edge functions can
evaluate them from `Context`.

For fan-out tasks, child tasks MUST write results to prefixed `Context` keys (e.g., `"analyst.fundamental"`,
`"analyst.sentiment"`, `"analyst.news"`, `"analyst.technical"`). The corresponding sync task MUST read all prefixed
results and merge them back into `TradingState`. Risk agents write directly to `TradingState` since they execute
sequentially and do not use fan-out prefixed keys.

Serialization failures MUST return `TradingError`. Deserialization failures MUST return `TradingError`.

#### Scenario: TradingState Round-Trips Through Context

- **WHEN** a `TradingState` is serialized into `Context` under the `"trading_state"` key, and subsequently
  deserialized by the next task
- **THEN** the deserialized `TradingState` is identical to the original, with no data loss across the
  serialization boundary

#### Scenario: Fan-Out Child Tasks Write To Prefixed Keys

- **WHEN** the 4 analyst fan-out child tasks complete execution
- **THEN** each child task writes its result to a prefixed `Context` key (`"analyst.fundamental"`,
  `"analyst.sentiment"`, `"analyst.news"`, `"analyst.technical"`), and the `AnalystSyncTask` reads all 4 prefixed
  keys and merges them back into `TradingState`

#### Scenario: Missing Context Key Returns Error

- **WHEN** a task attempts to deserialize `TradingState` from `Context` but the `"trading_state"` key is absent
- **THEN** the task returns a `TradingError` indicating the missing key

#### Scenario: Malformed JSON In Context Returns Error

- **WHEN** the `"trading_state"` key in `Context` contains invalid JSON
- **THEN** the task returns a `TradingError` wrapping the deserialization failure

### Requirement: Analyst Fan-Out Task Wrappers

The system MUST implement 4 analyst task wrappers (`FundamentalAnalystTask`, `SentimentAnalystTask`,
`NewsAnalystTask`, `TechnicalAnalystTask`) each implementing graph-flow's `Task` trait. Each wrapper MUST:

1. Deserialize `TradingState` from `Context`.
2. Invoke the corresponding analyst agent from `src/agents/analyst/`.
3. Write the result to a prefixed `Context` key (e.g., `"analyst.fundamental"`).
4. Record `AgentTokenUsage` from the agent's return value.
5. Return `TaskResult` with `NextAction::Continue`.

These 4 analyst task wrappers MUST be runnable in parallel because they do not depend on one another's outputs and
each produces one distinct slice of the Phase 1 analyst snapshot.

The system MUST implement `AnalystSyncTask` that reads all 4 prefixed analyst results from `Context`, merges them
into `TradingState`, and enforces the graceful degradation policy: 1 analyst failure continues with partial data;
2 or more analyst failures abort the cycle by returning `NextAction::End`.

#### Scenario: All Four Analysts Succeed

- **WHEN** all 4 analyst fan-out child tasks complete successfully and `AnalystSyncTask` reads their prefixed keys
- **THEN** `AnalystSyncTask` merges all 4 results into `TradingState`, re-serializes the updated state to
  `Context`, and returns `NextAction::Continue` to proceed to the researcher debate phase

#### Scenario: Analyst Fan-Out Executes In Parallel

- **WHEN** Phase 1 begins
- **THEN** `FundamentalAnalystTask`, `SentimentAnalystTask`, `NewsAnalystTask`, and `TechnicalAnalystTask` are
  dispatched concurrently rather than waiting for one another sequentially

#### Scenario: One Analyst Fails

- **WHEN** exactly 1 analyst child task fails (e.g., `SentimentAnalystTask` times out) and the other 3 succeed
- **THEN** `AnalystSyncTask` merges the 3 available results into `TradingState`, leaves the failed analyst's
  corresponding field as `None`, logs a warning, and returns `NextAction::Continue`

#### Scenario: Two Analysts Fail

- **WHEN** 2 analyst child tasks fail
- **THEN** `AnalystSyncTask` returns `NextAction::End` to abort the pipeline, as partial data from only 2 analysts
  is insufficient for reliable downstream synthesis

#### Scenario: Three Analysts Fail

- **WHEN** 3 analyst child tasks fail
- **THEN** `AnalystSyncTask` returns `NextAction::End` to abort the pipeline

#### Scenario: Four Analysts Fail

- **WHEN** all 4 analyst child tasks fail
- **THEN** `AnalystSyncTask` returns `NextAction::End` to abort the pipeline

## MODIFIED Requirements

### Requirement: Researcher Debate Task Wrappers

The system MUST implement `BullishResearcherTask`, `BearishResearcherTask`, and `DebateModeratorTask` each
implementing graph-flow's `Task` trait. Each node MUST perform exactly one real unit of work per invocation.
The debate cycle MUST be controlled by a conditional edge on `DebateModeratorTask`.

`BullishResearcherTask` MUST:
1. Deserialize `TradingState` from `Context` and read the current debate history.
2. Call `run_bullish_researcher_turn` — invoke the Bullish Researcher agent for one turn only.
3. The helper appends exactly one `DebateMessage` with role `"bullish_researcher"` to `TradingState::debate_history`.
4. Record `AgentTokenUsage` from the helper's return value.
5. Re-serialize `TradingState` to `Context`.
6. Return `TaskResult` with `NextAction::Continue`.

`BearishResearcherTask` MUST:
1. Deserialize `TradingState` from `Context` and read the latest bullish argument.
2. Call `run_bearish_researcher_turn` — invoke the Bearish Researcher agent for one turn only.
3. The helper appends exactly one `DebateMessage` with role `"bearish_researcher"` to `TradingState::debate_history`.
4. Record `AgentTokenUsage` from the helper's return value.
5. Re-serialize `TradingState` to `Context`.
6. Return `TaskResult` with `NextAction::Continue`.

`DebateModeratorTask` MUST:
1. Deserialize `TradingState` from `Context` and read the full debate history.
2. Call `run_debate_moderation` — invoke the Debate Moderator agent.
3. The helper writes `consensus_summary` to `TradingState`.
4. Increment the `debate_round` counter in `Context` (this is the round-completion checkpoint).
5. Re-serialize `TradingState` to `Context`.
6. On its final invocation (when the debate is complete and flow proceeds to `TraderTask`), save a phase snapshot
   via the `SnapshotStore`. Snapshot failure MUST return a task error — log-and-continue is not permitted.
7. Return `TaskResult` with `NextAction::Continue`.

Note: `debate_round` represents "rounds completed" — it is incremented by `DebateModeratorTask` after each full
bull+bear+moderator sequence. The conditional edge `debate_round < max_debate_rounds` correctly terminates after
the configured number of complete rounds.

A conditional edge from `DebateModeratorTask` MUST check if `debate_round < Config.llm.max_debate_rounds`: if true,
loop back to `BullishResearcherTask`; if false, continue to `TraderTask`.

#### Scenario: Three-Round Debate Produces Expected Output

- **WHEN** the debate cycle executes with `max_debate_rounds = 3` (default) and all LLM calls succeed
- **THEN** `TradingState::debate_history` contains 6 `DebateMessage` entries (alternating bullish/bearish),
  `TradingState::consensus_summary` is populated, and the conditional edge directs flow to `TraderTask`

#### Scenario: Single-Round Debate

- **WHEN** the debate cycle executes with `max_debate_rounds = 1`
- **THEN** `TradingState::debate_history` contains 2 `DebateMessage` entries, `consensus_summary` is populated,
  and flow continues to `TraderTask`

#### Scenario: Zero-Round Debate Routes Directly To Moderator

- **WHEN** `max_debate_rounds = 0`
- **THEN** the entry conditional edge from `AnalystSyncTask` directs flow to `DebateModeratorTask` directly
  (skipping `BullishResearcherTask` and `BearishResearcherTask`), `DebateModeratorTask` calls
  `run_debate_moderation` with empty debate history and produces a `consensus_summary` from analyst data alone,
  and flow continues to `TraderTask`

#### Scenario: Each Node Does Exactly One Unit Of Work

- **WHEN** a three-round debate executes
- **THEN** `BullishResearcherTask` is invoked 3 times, each time appending exactly one bullish `DebateMessage`;
  `BearishResearcherTask` is invoked 3 times, each time appending exactly one bearish `DebateMessage`;
  `DebateModeratorTask` is invoked 3 times with `debate_round` incrementing at each moderator checkpoint

#### Scenario: Conditional Edge Loops Correctly

- **WHEN** `max_debate_rounds = 5` and the debate has completed 3 rounds
- **THEN** the conditional edge on `DebateModeratorTask` evaluates `debate_round (3) < 5` as true and directs
  flow back to `BullishResearcherTask` for round 4

### Requirement: Risk Discussion Task Wrappers

The system MUST implement `AggressiveRiskTask`, `ConservativeRiskTask`, `NeutralRiskTask`, and `RiskModeratorTask`
each implementing graph-flow's `Task` trait. Each node MUST perform exactly one real unit of work per invocation.
The risk discussion cycle MUST be controlled by a conditional edge on `RiskModeratorTask`.

`AggressiveRiskTask` MUST:
1. Deserialize `TradingState` from `Context`.
2. Call `run_aggressive_risk_turn` — invoke the Aggressive Risk Agent for one turn only.
3. The helper writes `RiskReport` to `TradingState::aggressive_risk_report`.
4. Record `AgentTokenUsage` from the helper's return value.
5. Re-serialize `TradingState` to `Context`.
6. Return `TaskResult` with `NextAction::Continue`.

`ConservativeRiskTask` MUST:
1. Deserialize `TradingState` from `Context`.
2. Call `run_conservative_risk_turn` — invoke the Conservative Risk Agent for one turn only.
3. The helper writes `RiskReport` to `TradingState::conservative_risk_report`.
4. Record `AgentTokenUsage` from the helper's return value.
5. Re-serialize `TradingState` to `Context`.
6. Return `TaskResult` with `NextAction::Continue`.

`NeutralRiskTask` MUST:
1. Deserialize `TradingState` from `Context`.
2. Call `run_neutral_risk_turn` — invoke the Neutral Risk Agent for one turn only.
3. The helper writes `RiskReport` to `TradingState::neutral_risk_report`.
4. Record `AgentTokenUsage` from the helper's return value.
5. Re-serialize `TradingState` to `Context`.
6. Return `TaskResult` with `NextAction::Continue`.

`RiskModeratorTask` MUST:
1. Deserialize `TradingState` from `Context`.
2. Call `run_risk_moderation` — invoke the Risk Moderator agent.
3. The helper appends the synthesis to `TradingState::risk_discussion_history`.
4. Increment the `risk_round` counter in `Context` (this is the round-completion checkpoint).
5. Re-serialize `TradingState` to `Context`.
6. On its final invocation (when the discussion is complete and flow proceeds to `FundManagerTask`), save a phase
   snapshot via the `SnapshotStore`. Snapshot failure MUST return a task error — log-and-continue is not permitted.
7. Return `TaskResult` with `NextAction::Continue`.

Note: `risk_round` represents "rounds completed" — it is incremented by `RiskModeratorTask` after each full
Aggressive → Conservative → Neutral → Moderator sequence. The conditional edge `risk_round < max_risk_rounds`
correctly terminates after the configured number of complete rounds.

A conditional edge from `RiskModeratorTask` MUST check if `risk_round < Config.llm.max_risk_rounds`: if true, loop
back to `AggressiveRiskTask`; if false, continue to `FundManagerTask`.

#### Scenario: Two-Round Risk Discussion Completes

- **WHEN** the risk discussion cycle executes with `max_risk_rounds = 2` (default) and all LLM calls succeed
- **THEN** all 3 `RiskReport` fields are populated with the final round's reports,
  `TradingState::risk_discussion_history` contains the moderator's synthesis entries, and the conditional edge
  directs flow to `FundManagerTask`

#### Scenario: Each Node Does Exactly One Unit Of Work

- **WHEN** a two-round risk discussion executes
- **THEN** `AggressiveRiskTask` is invoked 2 times each writing one `aggressive_risk_report`;
  `ConservativeRiskTask` is invoked 2 times each writing one `conservative_risk_report`;
  `NeutralRiskTask` is invoked 2 times each writing one `neutral_risk_report`;
  `RiskModeratorTask` is invoked 2 times with `risk_round` incrementing at each moderator checkpoint

#### Scenario: Risk Conditional Edge Loops Correctly

- **WHEN** `max_risk_rounds = 3` and the discussion has completed 1 round
- **THEN** the conditional edge on `RiskModeratorTask` evaluates `risk_round (1) < 3` as true and directs flow
  back to `AggressiveRiskTask` for round 2

#### Scenario: Zero-Round Risk Routes Directly To Moderator

- **WHEN** `max_risk_rounds = 0`
- **THEN** the entry conditional edge from `TraderTask` directs flow to `RiskModeratorTask` directly (skipping
  `AggressiveRiskTask`, `ConservativeRiskTask`, and `NeutralRiskTask`), `RiskModeratorTask` calls
  `run_risk_moderation` with the trade proposal alone and produces a synthesis, and flow continues to
  `FundManagerTask`

### Requirement: Trader Task Wrapper

The system MUST implement `TraderTask` implementing graph-flow's `Task` trait. The wrapper MUST:

1. Deserialize `TradingState` from `Context`.
2. Invoke the Trader agent (which reads analyst outputs and `consensus_summary` to produce a `TradeProposal`).
3. Record `AgentTokenUsage` from the agent's return value.
4. Re-serialize `TradingState` to `Context`.
5. Save a phase snapshot via the `SnapshotStore`. Snapshot failure MUST return a task error.
6. Return `TaskResult` with `NextAction::Continue`.

#### Scenario: TraderTask Produces Proposal And Continues

- **WHEN** `TraderTask` executes with populated analyst data and `consensus_summary` in `TradingState`
- **THEN** the Trader agent writes a `TradeProposal` to `TradingState::trader_proposal`, the updated state is
  re-serialized to `Context`, a phase snapshot is saved, and execution continues to the risk discussion phase

#### Scenario: TraderTask Failure Propagates Error

- **WHEN** the Trader agent invocation fails with an LLM error or timeout
- **THEN** the task returns a `TradingError` that propagates through the graph-flow pipeline, halting execution

### Requirement: Fund Manager Task Wrapper

The system MUST implement `FundManagerTask` implementing graph-flow's `Task` trait. The wrapper MUST:

1. Deserialize `TradingState` from `Context`.
2. Invoke the Fund Manager agent (which reads `TradeProposal`, all `RiskReport` objects,
   `risk_discussion_history`, and supporting analyst context to produce `ExecutionStatus`).
3. Record `AgentTokenUsage` from the agent's return value.
4. Re-serialize `TradingState` to `Context`.
5. Save the final phase snapshot via the `SnapshotStore`. Snapshot failure MUST return a task error.
6. Return `TaskResult` with `NextAction::End` (terminal node).
7. The fund-manager rationale MUST NOT be logged at info level; only structured decision metadata
   (approve/reject, phase, task id) may appear in structured log events.

#### Scenario: FundManagerTask Produces ExecutionStatus And Ends

- **WHEN** `FundManagerTask` executes with a populated `TradeProposal` and all `RiskReport` fields in
  `TradingState`
- **THEN** the Fund Manager agent writes an `ExecutionStatus` to `TradingState`, the final phase snapshot is
  saved, and the task returns `NextAction::End` to terminate the pipeline

#### Scenario: Deterministic Rejection Path Through Wrapper

- **WHEN** both the Conservative and Neutral `RiskReport` objects have `flags_violation = true`
- **THEN** the Fund Manager agent's deterministic rejection rule still operates correctly through the task
  wrapper, producing a rejection `ExecutionStatus` and ending the pipeline

### Requirement: SQLite Phase Snapshot Storage

The system MUST implement a `SnapshotStore` backed by SQLite via `sqlx` that persists `TradingState` at phase
boundaries. The schema MUST include the following columns:

- `execution_id` (TEXT, NOT NULL)
- `phase_number` (INTEGER, NOT NULL)
- `phase_name` (TEXT, NOT NULL)
- `trading_state_json` (TEXT, NOT NULL)
- `token_usage_json` (TEXT, nullable)
- `created_at` (TEXT, NOT NULL, default `datetime('now')`)
- UNIQUE constraint on `(execution_id, phase_number)`

The `SnapshotStore` MUST provide `save_snapshot` and `load_snapshot` operations. `load_snapshot` MUST be able to
return both the deserialized `TradingState` and any persisted token-usage payload for that phase. Schema creation MUST
use `sqlx` migrations from the `migrations/` directory (`sqlx::migrate!`). Inline schema duplication in Rust MUST NOT
be used as the authoritative schema source.

The SQLite file path MUST be configurable. When no explicit path is configured, the snapshot store MUST default to
`$HOME/.scorpio-analyst/phase_snapshots.db`. If the `$HOME/.scorpio-analyst` directory does not exist, the snapshot
store MUST create it before opening or migrating the database.

The SQLite migration MUST live in a root-level `migrations/` directory owned by this change (for example,
`migrations/0001_create_phase_snapshots.sql`).

#### Scenario: Phase Snapshot Saved And Loaded Successfully

- **WHEN** a `TradingState` is saved via `save_snapshot` with a given `execution_id` and `phase_number`, then
  loaded via `load_snapshot` with the same identifiers
- **THEN** the loaded `TradingState` is identical to the saved state, confirming lossless round-trip through
  SQLite storage

#### Scenario: Default Snapshot Path Is Used

- **WHEN** `SnapshotStore` is constructed without an explicit SQLite file path
- **THEN** it resolves the database path to `$HOME/.scorpio-analyst/phase_snapshots.db`

#### Scenario: Missing Parent Directory Is Created

- **WHEN** `SnapshotStore` is constructed without an explicit SQLite file path and `$HOME/.scorpio-analyst` does not
  yet exist
- **THEN** the snapshot store creates `$HOME/.scorpio-analyst` before opening or migrating
  `phase_snapshots.db`

#### Scenario: Explicit Snapshot Path Overrides Default

- **WHEN** `SnapshotStore` is constructed with an explicit SQLite file path
- **THEN** it uses that explicit path instead of `$HOME/.scorpio-analyst/phase_snapshots.db`

#### Scenario: Duplicate Phase Number Handled

- **WHEN** `save_snapshot` is called twice for the same `execution_id` and `phase_number`
- **THEN** the store either performs an upsert (replacing the previous snapshot) or returns an error indicating
  the duplicate, without corrupting the database

#### Scenario: Missing Snapshot Returns None

- **WHEN** `load_snapshot` is called with an `execution_id` and `phase_number` that do not exist in the database
- **THEN** the operation returns `None` rather than an error

#### Scenario: Token Usage Stored Alongside State

- **WHEN** `save_snapshot` is called with both a `TradingState` JSON blob and a `token_usage_json` value
- **THEN** the token usage JSON is persisted in the same row and is retrievable via `load_snapshot`

#### Scenario: Snapshot Failure Returns Task Error

- **WHEN** `save_snapshot` returns an error during a workflow task execution
- **THEN** the task returns an error and the pipeline halts — the task MUST NOT log and continue

### Requirement: Pipeline Token Accounting

Each task wrapper MUST capture `AgentTokenUsage` from the wrapped agent's return value. At phase boundaries,
accumulated `AgentTokenUsage` entries MUST be finalized into `PhaseTokenUsage` (including phase name, timing, and
all agent entries) and appended to `TradingState.token_usage.phase_usage`. Token accounting MUST be written back
into `TradingState` during the pipeline; it MUST NOT remain only in local variables.

For cyclic researcher and risk phases, the pipeline MUST preserve multiple `PhaseTokenUsage` entries so individual
rounds and the final moderator step are tracked separately (for example, `Researcher Debate Round 1`,
`Researcher Debate Moderation`, `Risk Discussion Round 1`, `Risk Discussion Moderation`).

The total cycle token usage MUST be computed at pipeline completion by summing all `PhaseTokenUsage` entries.

If a provider does not return authoritative token counts, `AgentTokenUsage::unavailable` may be used, but the
corresponding `PhaseTokenUsage` entry MUST still be materialized and appended.

#### Scenario: Full Pipeline Produces Phase Token Usage

- **WHEN** a full 5-phase pipeline execution completes successfully
- **THEN** `TradingState.token_usage.phase_usage` contains `PhaseTokenUsage` entries for the analyst phase,
  each configured researcher/risk round, each moderation step, the trader phase, and the fund manager phase, each
  with correct agent-level entries

#### Scenario: Phase Timing Reflects Wall-Clock Duration

- **WHEN** a phase completes execution
- **THEN** the corresponding `PhaseTokenUsage` entry records a `duration` that reflects the wall-clock time
  elapsed from phase start to phase completion

#### Scenario: Agent Tokens Attributed To Correct Phase

- **WHEN** the Bullish Researcher agent records `AgentTokenUsage` during Phase 2
- **THEN** that entry appears in the Phase 2 (Researcher Debate) `PhaseTokenUsage` and not in any other phase's
  entries

#### Scenario: Unavailable Tokens Still Materialize Phase Entry

- **WHEN** a provider returns unavailable token counts for an agent in Phase 3
- **THEN** a `PhaseTokenUsage` entry for Phase 3 is still appended to `TradingState.token_usage.phase_usage`
  with the `AgentTokenUsage::unavailable` entry included

### Requirement: Pipeline Public API

The system MUST provide a `TradingPipeline` struct with:

- `new(config, finnhub, yfinance, snapshot_store, handle)` — constructor that builds the graph-flow graph topology
  using the existing data clients and provider helper functions already present in the codebase.
- `run_analysis_cycle(&self, state: TradingState) -> Result<TradingState>` — executes the full 5-phase pipeline.

The `run_analysis_cycle` function MUST:

1. Generate a fresh `Uuid` for the cycle and write it to the working copy of `TradingState.execution_id`
   (overwriting any caller-supplied value).
2. Create an `InMemorySessionStorage` instance.
3. Seed graph-flow `Context` with the serialized `TradingState` under the `"trading_state"` key.
4. Seed `Context` with `"max_debate_rounds"` and `"max_risk_rounds"` from `Config.llm`.
5. Execute `FlowRunner` to completion.
6. Extract and return the final `TradingState` from `Context`.

#### Scenario: Full Pipeline Execution Returns Updated State

- **WHEN** `run_analysis_cycle` is invoked with an initial `TradingState` containing the target asset symbol
- **THEN** the returned `TradingState` has all fields populated (analyst outputs, consensus summary, trade
  proposal, risk reports, execution status, token usage) reflecting the completed 5-phase analysis

#### Scenario: Pipeline Failure Propagates Error

- **WHEN** any phase within the pipeline fails with a `TradingError`
- **THEN** `run_analysis_cycle` returns the `TradingError` rather than a partial `TradingState`

#### Scenario: Fresh Execution ID Generated Per Invocation

- **WHEN** `run_analysis_cycle` is called multiple times
- **THEN** each invocation generates a distinct UUID `execution_id` written to `TradingState.execution_id`
  before the graph starts, ensuring phase snapshots from different runs do not collide even if the caller
  supplies the same initial state

### Requirement: FlowRunner Error Propagation

When a `Task::run` implementation returns an error, the `FlowRunner` MUST propagate it as a pipeline failure rather
than silently swallowing it. Graph-flow error types MUST be mapped to `TradingError` before returning from
`run_analysis_cycle`. The `TradingError` MUST preserve the original error context (phase name, task name, and
underlying cause), using `TradingError::GraphFlow { phase, task, cause }` when a new graph-orchestration-specific
variant is required.

Graph-flow errors MUST NOT collapse to generic `step_N` labels. When the real task name and phase are known at the
point of failure, they MUST be included in the `TradingError::GraphFlow` fields. Workflow-surfaced error messages
MUST apply the same sanitization posture used for provider-layer errors, avoiding accidental leakage of verbose
model output text.

#### Scenario: Task Error Propagates With Real Task Identity

- **WHEN** a task wrapper's `Task::run` implementation returns an error (e.g., LLM timeout in
  `BullishResearcherTask`)
- **THEN** the `FlowRunner` halts execution, the error is mapped to a `TradingError::GraphFlow` variant with
  the real task id (`"bullish_researcher"`) and phase name (`"researcher_debate"`) in the structured fields,
  and `run_analysis_cycle` returns the `TradingError`

#### Scenario: Graph-Flow Error Mapped To TradingError

- **WHEN** the `FlowRunner` itself returns an internal graph-flow error (e.g., missing task node, session storage
  failure)
- **THEN** `run_analysis_cycle` maps the graph-flow error to `TradingError` and returns it, rather than exposing
  graph-flow error types to callers

### Requirement: Workflow Module Boundary

This capability's implementation MUST remain centered on orchestration concerns within `src/workflow/`. The only
permitted modifications to agent files are the narrowly-scoped single-step helper additions explicitly approved as
cross-owner touch-points for this remediation.

The permitted cross-owner changes are:

- Adding `graph-flow`, `sqlx`, and `async-trait` to `Cargo.toml` (owned by `add-project-foundation`).
- Replacing the empty `src/workflow/mod.rs` skeleton (owned by `add-project-foundation`).
- Adding `TradingError::GraphFlow { phase, task, cause }` to `src/error.rs` (owned by
  `add-project-foundation`).
- Updating exhaustive `TradingError` handling in `src/providers/factory.rs` (owned by `add-llm-providers`) so the
  new graph-flow error variant is classified correctly.
- Adding single-step public helpers to `src/agents/researcher/mod.rs` (owned by `add-researcher-debate`):
  `run_bullish_researcher_turn`, `run_bearish_researcher_turn`, `run_debate_moderation`.
- Adding single-step public helpers to `src/agents/risk/mod.rs` (owned by `add-risk-management`):
  `run_aggressive_risk_turn`, `run_conservative_risk_turn`, `run_neutral_risk_turn`, `run_risk_moderation`.
- Adding a shared cached-news prefetch helper to `src/agents/analyst/mod.rs` (owned by `add-analyst-team`).

No other agent or state files may be modified.

#### Scenario: Downstream CLI Imports Pipeline

- **WHEN** the downstream CLI module imports the pipeline
- **THEN** it uses `use scorpio_analyst::workflow::{TradingPipeline, ...}` and receives the pipeline struct and
  public API through the workflow module path

#### Scenario: Agent Module Core Logic Unchanged

- **WHEN** the graph-orchestration remediation is applied
- **THEN** only the approved narrow helper additions are made to agent modules; core agent logic, existing public
  APIs, and all state type definitions remain unmodified


