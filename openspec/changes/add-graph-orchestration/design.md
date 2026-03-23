# Design for `add-graph-orchestration`

## Context

All five agent teams are implemented and expose well-defined `run_*` entry points: `add-analyst-team` (Phase 1),
`add-researcher-debate` (Phase 2), `add-trader-agent` (Phase 3), `add-risk-management` (Phase 4), and
`add-fund-manager` (Phase 5). The `src/workflow/mod.rs` file is currently an empty skeleton containing only the
comment `// Populated by add-graph-orchestration`.

This change introduces the central orchestration layer that wires all 5 phases into a `graph-flow` directed-graph
pipeline with 11 task nodes. The project uses the forked `graph-flow` repository at
`BigtoC/rs-graph-llm` branch `feature/update-rig-version`, which updates the `rig` integration to work with the
project's `rig-core 0.32`. That lets the orchestration layer use graph-flow's `rig` support directly while still
wrapping each agent/team in explicit `Task` implementations.

Additional dependencies: `sqlx` with SQLite for phase snapshot persistence (migration-friendly path to Postgres),
and `async-trait` for the graph-flow `Task` trait.

**Stakeholders:** `add-cli` (invokes `run_analysis_cycle()` from CLI subcommands and streams workflow progress),
`add-backtesting` (replays the pipeline over historical data). All five agent changes are upstream dependencies.

## Goals / Non-Goals

- **Goals:**
    - Wire all 5 phases into a graph-flow directed graph with 11 task nodes connected via `GraphBuilder`.
    - Implement the Context-as-envelope state bridging pattern: serialize full `TradingState` into graph-flow
      `Context` as a single JSON blob under the key `"trading_state"`.
    - Provide SQLite-based phase snapshot storage via `sqlx` with a `phase_snapshots` table, using a
      migration-friendly schema that can later target Postgres.
    - Use `InMemorySessionStorage` for graph-flow session management in the MVP.
    - Implement fan-out parallelism for the analyst phase (4 analysts via `FanOutTask`), since the
      upstream analyst-team capability already defines them as independent concurrent tasks.
    - Implement sequential execution for risk agents within rounds (Aggressive → Conservative → Neutral), with
      cycling between rounds via conditional edges and round counters stored in Context. Sequential ordering is
      required because each risk agent's prompt references the other agents' latest same-round views.
    - Implement cyclic debate loops for the researcher debate phase via conditional edges with round counters
      stored in Context.
    - Aggregate token accounting at phase boundaries into `PhaseTokenUsage` entries appended to
      `TradingState.token_usage.phase_usage`, preserving per-round entries for cyclic researcher and risk phases.
    - Enforce graceful degradation in `AnalystSyncTask`: 1 analyst failure continues with partial data, 2 or more
      failures abort the cycle.
    - Expose a public `run_analysis_cycle(&self, state: TradingState) -> Result<TradingState>` entry point for
      downstream consumers.
    - Emit structured `tracing` spans/events for phase transitions, round boundaries, task execution, snapshot saves,
      and failures so downstream CLI/TUI layers can stream progress.

- **Non-Goals:**
    - Reverting to the upstream graph-flow release without the forked `rig-core 0.32` compatibility.
    - PostgreSQL storage (deferred to future; SQLite for MVP).
    - Broad agent refactoring — only narrow single-step helper additions to researcher, risk, and analyst modules.
    - CLI integration (owned by `add-cli`).
    - Backtesting integration (owned by `add-backtesting`).
    - Per-agent timeout configuration (uses existing per-agent timeouts from the agent layer).
    - Memory/past experience retrieval (deferred to future enhancement).

## Remediation Addendum

The initial implementation compiled and passed tests but deviated from several approved behaviors:

- `BullishResearcherTask` called `run_researcher_debate` with `max_rounds=1`, running the full loop (bull + bear + moderator) inside one node. `BearishResearcherTask` and `DebateModeratorTask` were no-op placeholders.
- `AggressiveRiskTask` called `run_risk_discussion` with `max_rounds=1`, running the full risk round. `ConservativeRiskTask`, `NeutralRiskTask`, and `RiskModeratorTask` were no-op placeholders.
- `run_analysis_cycle` used a time-based session ID, not a per-run generated UUID written to `TradingState.execution_id`.
- Snapshot persistence failures logged a warning and continued instead of failing the task.
- Per-phase token accounting was not written back into `TradingState.token_usage`.
- No integration tests covered `run_analysis_cycle` end-to-end.

### Corrected per-node execution model

**Phase 2 — Researcher Debate:**

The researcher module exposes three narrowly-scoped public helpers:

- `run_bullish_researcher_turn(state, handle, llm_config)` — executes exactly one bullish turn, appends one `DebateMessage` to `state.debate_history`, returns `AgentTokenUsage`.
- `run_bearish_researcher_turn(state, handle, llm_config)` — executes exactly one bearish turn, appends one `DebateMessage` to `state.debate_history`, returns `AgentTokenUsage`.
- `run_debate_moderation(state, handle, llm_config)` — executes the moderator synthesis, sets `state.consensus_summary`, returns `AgentTokenUsage`.

`BullishResearcherTask` calls `run_bullish_researcher_turn`. `BearishResearcherTask` calls `run_bearish_researcher_turn`. `DebateModeratorTask` calls `run_debate_moderation` and owns the round-completion checkpoint.

The `debate_round` counter increments at the `DebateModeratorTask` checkpoint (not at `BullishResearcherTask` entry). This ensures the conditional edge evaluates correctly after a complete bull+bear+moderator sequence.

If `max_debate_rounds = 0`, the graph routes directly to `DebateModeratorTask`, which calls `run_debate_moderation` on the empty debate history to produce a consensus from analyst data alone.

**Phase 4 — Risk Discussion:**

The risk module exposes four narrowly-scoped public helpers:

- `run_aggressive_risk_turn(state, handle, llm_config)` — executes exactly one aggressive risk turn, writes `state.aggressive_risk_report`, returns `AgentTokenUsage`.
- `run_conservative_risk_turn(state, handle, llm_config)` — executes one conservative turn, writes `state.conservative_risk_report`, returns `AgentTokenUsage`.
- `run_neutral_risk_turn(state, handle, llm_config)` — executes one neutral turn, writes `state.neutral_risk_report`, returns `AgentTokenUsage`.
- `run_risk_moderation(state, handle, llm_config)` — executes the moderator synthesis, appends to `state.risk_discussion_history`, returns `AgentTokenUsage`.

`RiskModeratorTask` owns the round-completion checkpoint and increments `risk_round` after the full Aggressive → Conservative → Neutral → Moderator sequence.

### Execution identity and mandatory snapshots

`run_analysis_cycle` generates a fresh `Uuid` at the start of each invocation and writes it to the working copy of `TradingState.execution_id` before the graph starts. Caller-provided execution IDs are overwritten.

Snapshot persistence is mandatory. If `save_snapshot` fails, the task returns an error and the pipeline halts. Log-and-continue is not permitted for snapshot failures.

### Migration-driven snapshot schema

`SnapshotStore` uses `sqlx::migrate!` to initialize the `phase_snapshots` schema from the `migrations/` directory. Inline schema definitions in Rust are removed. This eliminates drift between the SQL migration file and the Rust initialization path.

### Token accounting

The workflow layer accumulates `AgentTokenUsage` entries during each phase and writes them into `TradingState.token_usage` at phase boundaries:

- Phase 1: one `PhaseTokenUsage` entry for the analyst fan-out (four analyst agent entries).
- Phase 2: separate `PhaseTokenUsage` entries per debate round (`Researcher Debate Round N`) plus one for moderation (`Researcher Debate Moderation`).
- Phase 3: one `PhaseTokenUsage` entry for the trader.
- Phase 4: separate entries per risk round (`Risk Discussion Round N`) plus one for moderation (`Risk Discussion Moderation`).
- Phase 5: one `PhaseTokenUsage` entry for the fund manager.

Providers that do not return authoritative counts may still use `AgentTokenUsage::unavailable`, but `PhaseTokenUsage` entries must be materialized and totals must be internally consistent.

### Error mapping

`TradingError::GraphFlow` carries real phase and task identity. Errors must not collapse to generic `step_N` labels when the real task and phase are known. Workflow error messages apply the same sanitization posture used for provider-layer errors.

## Architectural Overview

```
src/workflow/
├── mod.rs              <- Public module root, re-exports
├── pipeline.rs         <- TradingPipeline struct, graph construction, FlowRunner execution
├── tasks.rs            <- 14 Task trait implementations (11 graph nodes; 4 analyst tasks nested in FanOutTask)
├── snapshot.rs         <- SQLite SnapshotStore for phase snapshots
└── context_bridge.rs   <- TradingState ↔ graph-flow Context serialization helpers

migrations/
└── 0001_create_phase_snapshots.sql <- SQLx migration for phase_snapshots table
```

### Graph Topology (11 Nodes)

```
                         ┌──────────────────────────────────┐
                         │  Phase 1: Analyst Fan-Out        │
                         │                                  │
                         │  FanOutTask("analyst_fanout")    │
                         │   ├─ FundamentalAnalystTask      │
                         │   ├─ SentimentAnalystTask        │
                         │   ├─ NewsAnalystTask             │
                         │   └─ TechnicalAnalystTask        │
                         └──────────────┬───────────────────┘
                                        │
                                        ▼
                              ┌──────────────────┐
                              │ AnalystSyncTask  │
                              │ (merge + degrade)│
                              └────────┬─────────┘
                                       │
                               conditional edge:
                               max_debate_rounds > 0?
                                   │           │
                             yes   │           │ no (skip to moderator)
                                   ▼           │
              ┌────────────────────────────────│────────────────┐
              │  Phase 2: Researcher Debate    │  (cyclic)      │
              │                                │                │
              │  ┌──────────────────────┐      │                │
         ┌───►│  BullishResearcherTask  │      │                │
         │    │  └──────────┬───────────┘      │                │
         │    │             ▼                  │                │
         │    │  ┌───────────────────────┐     │                │
         │    │  │ BearishResearcherTask │     │                │
         │    │  └──────────┬────────────┘     │                │
         │    │             ▼                  │                │
         │    │  ┌──────────────────────┐      │                │
         │    │  │ DebateModeratorTask  │──── conditional edge  │
         │    │  └──────────────────────┘     debate_round <    │
         │    │                               max_debate_rounds?│
         │    └─────────────────────────────────────────────────┘
         │              │ no (continue)
         │              ▼ ◄───────────────────┘
         │    ┌─────────────────┐
         │    │  Phase 3: Trader │
         │    │  TraderTask      │
         │    └────────┬────────┘
         │             │
         │    conditional edge:
         │    max_risk_rounds > 0?
         │        │           │
         │  yes   │           │ no (skip to moderator)
         │        ▼           │
         │    ┌──────────────│───────────────────────────────────┐
         │    │  Phase 4:    │  Risk Discussion (cyclic)         │
         │    │              │                                   │
         │    │  AggressiveRiskTask                              │
         │    │       │                                          │
         │    │       ▼                                          │
         │    │  ConservativeRiskTask                            │
         │    │       │                                          │
         │    │       ▼                                          │
         │    │  NeutralRiskTask                                 │
         │    │       │                                          │
         │    │       ▼                                          │
         │    │  ┌──────────────────────┐                        │
         │    │  │ RiskModeratorTask     │──── conditional edge  │
         │    │  └──────────────────────┘     risk_round <       │
         │    │       │ yes (loop back)       max_risk_rounds?   │
         │    │       └──► AggressiveRiskTask                    │
         │    └──────────────────────────────────────────────────┘
         │              │ no (continue)
         │              ▼ ◄───────────────────┘
         │    ┌─────────────────────┐
         │    │  Phase 5: Fund Mgr  │
         │    │  FundManagerTask    │
         │    └─────────────────────┘
         │              │
         │              ▼
         │           [ End ]
```

Phase 2 conditional edge: after `DebateModeratorTask`, if `debate_round < max_debate_rounds`, the edge loops back
to `BullishResearcherTask`; otherwise execution continues to Phase 3. Phase 4 conditional edge: after
`RiskModeratorTask`, if `risk_round < max_risk_rounds`, the edge loops back to `AggressiveRiskTask`; otherwise
execution continues to Phase 5. Within each risk round, agents execute sequentially (Aggressive → Conservative →
Neutral → Moderator) because each agent's prompt references the other agents' latest same-round views.

Zero-round handling: a conditional edge from `AnalystSyncTask` checks `max_debate_rounds > 0`; if false, execution
skips the researcher debate tasks and proceeds directly to `DebateModeratorTask` (not `TraderTask`), so the moderator
still produces a consensus summary from analyst data alone. Similarly, a conditional edge from `TraderTask` checks
`max_risk_rounds > 0`; if false, execution skips the risk persona tasks and proceeds directly to `RiskModeratorTask`
(not `FundManagerTask`), so the moderator still produces a synthesis from the trade proposal alone. These entry-point
conditional edges are in addition to the moderator exit conditional edges.

### Context-as-Envelope Pattern

`TradingState` is serialized to JSON and stored in graph-flow `Context` under the key `"trading_state"`. Each task
follows a uniform access pattern:

1. **Deserialize**: Read `"trading_state"` from Context, deserialize into typed `TradingState`.
2. **Operate**: Invoke the wrapped agent, mutate the relevant `TradingState` fields.
3. **Re-serialize**: Write the updated `TradingState` back to Context as JSON.

Fan-out child tasks use prefixed Context keys to write individual results without contention:
- Analyst fan-out: `"analyst.fundamental"`, `"analyst.sentiment"`, `"analyst.news"`, `"analyst.technical"`

Risk agents execute sequentially within each round and read/write directly to `TradingState` via the
`"trading_state"` key (no prefixed keys needed). Each risk agent sees the previous agent's updates immediately
because they run in series.

The `AnalystSyncTask` reads the prefixed analyst keys, merges results back into `TradingState`, and writes the
unified state to `"trading_state"`. It also enforces graceful degradation (1 analyst failure continues, 2+ abort).
This fan-out is safe because the four analyst tasks are independent and each writes one distinct analyst-owned field
(`fundamental_metrics`, `market_sentiment`, `macro_news`, `technical_indicators`) rather than mutating shared
conversation history.

### Config-in-Context

`Config.llm.max_debate_rounds` and `Config.llm.max_risk_rounds` are stored in Context at pipeline initialization
under the keys `"max_debate_rounds"` and `"max_risk_rounds"` (as integer values). This allows conditional edge
functions to read round limits directly from Context without capturing `Config` references, which would complicate
lifetime management in graph-flow's closure-based edge API.

### Phase Snapshot Strategy

SQLite via `sqlx` with a `phase_snapshots` table:

```sql
CREATE TABLE phase_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    execution_id TEXT NOT NULL,
    phase_number INTEGER NOT NULL,
    phase_name TEXT NOT NULL,
    trading_state_json TEXT NOT NULL,
    token_usage_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(execution_id, phase_number)
);
```

Snapshots are taken at each phase boundary (between phases), yielding 5 snapshots per complete analysis cycle:
`AnalystSyncTask` (end of Phase 1), `DebateModeratorTask` final invocation only (end of Phase 2), `TraderTask`
(end of Phase 3), `RiskModeratorTask` final invocation only (end of Phase 4), and `FundManagerTask` (end of
Phase 5). Intermediate debate/risk round iterations do not trigger snapshots. The `SnapshotStore` struct manages
SQLite connection pooling and provides `save_snapshot` / `load_snapshot` methods.

The SQLite file path is configurable. When no explicit path is provided, `SnapshotStore` resolves the default path to
`$HOME/.scorpio-analyst/phase_snapshots.db`. If the `$HOME/.scorpio-analyst` directory is absent, `SnapshotStore`
creates it before opening or migrating the database. This keeps the default audit trail location stable for local
users while still allowing tests, CI, and advanced deployments to override the database path.
The `sqlx` multi-database abstraction provides a migration-friendly path to Postgres when the project scales beyond
single-node SQLite.

### Token Accounting Flow

Each task wrapper captures `AgentTokenUsage` from the wrapped agent's return value. At phase boundaries,
accumulated entries are finalized into `PhaseTokenUsage` with phase timing (wall-clock start/end) and appended to
`TradingState.token_usage.phase_usage`. The naming convention follows the existing upstream contracts:
`"Analyst Fan-Out"`, `"Researcher Debate Round 1"`, `"Researcher Debate Round 2"`,
`"Researcher Debate Moderation"`, `"Trader Synthesis"`, `"Risk Discussion Round 1"`,
`"Risk Discussion Round 2"`, `"Risk Discussion Moderation"`, `"Fund Manager Decision"`.
Total cycle usage is computed at the end by summing all phase entries while respecting
`AgentTokenUsage.token_counts_available` when authoritative token counts are unavailable.

### TradingPipeline Construction

`TradingPipeline` is the public-facing struct for downstream consumers:

- `TradingPipeline::new(config: Config, finnhub: FinnhubClient, yfinance: YFinanceClient, snapshot_store:
  SnapshotStore) -> Result<Self>` — constructs the graph via `GraphBuilder`, registers all 11 task nodes and their
  edges (including conditional edges for cyclic phases), stores the provided data clients and `SnapshotStore`, seeds
  Context with `"max_debate_rounds"` and `"max_risk_rounds"` from `Config`, and prepares the immutable resources
  required for per-run `FlowRunner` execution. Individual task wrappers use the existing provider-layer helpers
  (`create_completion_model`, `run_trader`, `run_fund_manager`, etc.) rather than a nonexistent `ProviderFactory`
  type.
- `run_analysis_cycle(&self, state: TradingState) -> Result<TradingState>` — creates a new session, generates a
  unique `execution_id`, seeds graph-flow `Context` with the serialized `TradingState`, runs the `FlowRunner` to
  completion, extracts and deserializes the final `TradingState` from Context, and returns it.

### Observability

The pipeline emits structured `tracing` spans/events at these boundaries:

- analysis-cycle start/end (`execution_id`, symbol, target date)
- phase start/end (phase number, phase name, elapsed time)
- debate/risk round start/end (`debate_round` / `risk_round`)
- task start/success/failure (task id, mapped error context)
- snapshot save/load operations (execution_id, phase number)

These events are required by the PRD's real-time CLI/TUI streaming model and keep the orchestration layer aligned with
the foundation observability contract.

## Key Decisions

- **Forked graph-flow with `rig` support**: the project uses the forked `BigtoC/rs-graph-llm` branch
  `feature/update-rig-version`, whose `graph-flow` crate exposes `rig = ["dep:rig-core"]` against the project's
  `rig-core 0.32`. This preserves direct graph-flow/rig interoperability without needing to maintain local no-`rig`
  workarounds. *Alternative*: stay on upstream `graph-flow = { version = "0.4", default-features = false }` and avoid
  the `rig` feature — rejected because the fork already resolves the compatibility blocker.

- **Context-as-envelope serialization**: Full `TradingState` is serialized as a single JSON blob in graph-flow
  `Context`, deserialized and re-serialized at each task boundary. *Alternative*: store individual `TradingState`
  fields as separate Context keys — rejected because `TradingState` has deep nested structures and per-field
  management would be fragile, verbose, and prone to inconsistency.

- **SQLite via sqlx for snapshots**: Provides ACID guarantees, query capability, and a migration-friendly path to
  Postgres via `sqlx`'s multi-database support. *Alternative*: JSON files to disk (PRD's original approach) —
  rejected because SQLite provides better reliability and queryability. *Alternative*: skip persistence entirely —
  rejected because the audit trail is a PRD requirement.

- **InMemorySessionStorage for MVP**: graph-flow provides `InMemorySessionStorage` out of the box for managing
  internal execution state (current position, step history). `PostgresSessionStorage` is deferred to post-MVP. The
  session storage is separate from our phase snapshots — it tracks graph-flow's execution progress, not the
  `TradingState` audit trail.

- **11 task nodes wrapping individual agents**: Each agent gets its own `Task` wrapper rather than wrapping the
  aggregate `run_*` functions. This gives graph-flow visibility into each execution step, enables fine-grained
  conditional edges for debate/risk cycling, and preserves the fan-out topology in the graph definition.
  *Alternative*: 5 coarse tasks wrapping `run_*` functions — rejected because it hides internal parallelism and
  cycling from graph-flow, defeating the purpose of using a graph execution engine.

- **Fan-out via graph-flow FanOutTask (Phase 1 analysts only)**: Phase 1 analysts use `FanOutTask` for parallel
  execution via `tokio::spawn`. This matches the upstream `add-analyst-team` requirement that all four analysts run
  concurrently, and it is correct because the tasks are independent and produce separate slices of the analyst
  snapshot. Child tasks write to prefixed Context keys; `AnalystSyncTask` merges results.
  *Alternative*: manual `tokio::spawn` in a single task — rejected because `FanOutTask` provides built-in parallel
  orchestration with prefix namespacing and consistent error propagation.

- **Sequential execution for risk agents**: Phase 4 risk agents execute sequentially within each round
  (Aggressive → Conservative → Neutral) rather than in parallel, because the upstream `add-risk-management` spec
  mandates that each agent's prompt references the other agents' latest same-round views. This is fundamental to
  the progressive refinement of the risk assessment. *Alternative*: fan-out parallelism via `FanOutTask` — rejected
  because the prompt dependency between risk agents requires sequential execution to ensure each agent sees the
  previous agent's output from the current round.

- **Conditional edges for cyclic loops**: Researcher debate and risk discussion use `add_conditional_edge` with
  round-counter conditions stored in Context. This keeps each debate/risk round visible as a distinct graph
  execution step. Entry-point conditional edges at `AnalystSyncTask` and before `AggressiveRiskTask` handle the
  zero-round case (`max_debate_rounds = 0` or `max_risk_rounds = 0`) by skipping the cyclic phase entirely.
  *Alternative*: loop inside a single task — rejected to maintain graph-flow's visibility and audit capability over
  each cycle iteration.

- **Phase snapshots at boundaries, not per-step**: Snapshots are taken between phases (5 snapshots per cycle), not
  after every individual task step. This keeps storage reasonable while providing phase-level audit trail.
  *Alternative*: per-step snapshots — rejected as excessive I/O and storage for 11+ steps per cycle with minimal
  additional diagnostic value.

## Risks / Trade-offs

- **Serialization overhead**: Full `TradingState` JSON serialization at every task boundary adds latency.
  Mitigation: `TradingState` is relatively small (analyst reports are text, not large binary data); serialization
  cost is negligible compared to LLM call latency, which dominates each task's execution time.

- **graph-flow API stability**: graph-flow 0.4.0 is a relatively young crate with a small user base. Mitigation:
  we pin the exact version; the `Task` trait surface is small and stable; the wrapper layer in `tasks.rs` isolates
  the rest of the codebase from graph-flow API changes.

- **Context key collisions**: Fan-out prefix keys could collide if naming is inconsistent. Mitigation: use
  well-defined constant prefixes (`"analyst."`) declared in `context_bridge.rs`; `AnalystSyncTask` validates that
  expected keys exist before merging.

- **SQLite write contention**: Under backtesting with parallel analysis cycles, SQLite's single-writer model could
  become a bottleneck. Mitigation: each execution gets a unique `execution_id` and snapshots are append-only; for
  the MVP single-cycle execution this is not an issue; the Postgres migration path addresses this for backtesting
  workloads.

- **Debate/risk round state in Context**: Round counters stored in graph-flow Context must be correctly
  incremented by task wrappers and read by conditional edge functions. Off-by-one errors could cause infinite loops
  or premature termination. Mitigation: unit tests verify round counting for both debate and risk phases; the
  conditional edge functions are simple integer comparisons against values read from Context (`"max_debate_rounds"`
  and `"max_risk_rounds"`).

- **FlowRunner error propagation**: When a `Task::run` implementation returns an error, graph-flow's `FlowRunner`
  propagates it as a graph execution error and halts the pipeline. The `TradingPipeline` maps graph-flow errors to
  `TradingError` variants, preserving the original error context for diagnostics. Tasks that need graceful
  degradation (e.g., `AnalystSyncTask`) handle partial failures internally before returning `Ok`.

## Open Questions

None — all design decisions were resolved during the brainstorming phase.
