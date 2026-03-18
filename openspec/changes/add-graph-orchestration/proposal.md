# Change: Add Graph-Flow 5-Phase Pipeline Orchestration

## Why

The scorpio-analyst trading system currently has all five agent teams implemented as independent
modules (`analyst`, `researcher`, `trader`, `risk`, `fund_manager`), each exposing well-defined
`run_*` entry points. However, there is no orchestration layer to wire these phases into a coherent
end-to-end pipeline. The `src/workflow/mod.rs` file is an empty skeleton placeholder.

The PRD mandates `graph-flow` as the stateful directed-graph execution engine, providing:
- **Fan-out parallelism** for the analyst phase via `FanOutTask`
- **Sequential risk assessment** with per-round Aggressive → Conservative → Neutral ordering
- **Cyclic debate loops** for researcher and risk discussion phases via conditional edges
- **Session management** with future PostgreSQL migration path
- **Deterministic execution topology** preventing agents from deviating into uncontrolled loops
- **Phase-level audit trail** via state snapshots at each phase boundary

Without this orchestration layer, no analysis cycle can be triggered — it is the central glue
connecting data ingestion, dialectical evaluation, synthesis, risk assessment, and final execution.

## What Changes

### New Dependency
- Add `graph-flow` from the forked repository branch that updates `rig-core` compatibility
  (`BigtoC/rs-graph-llm`, branch `feature/update-rig-version`), enabling the `rig` feature with
  the project's `rig-core 0.32`
- Add `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }` for SQLite-based phase
  snapshot storage (migration-friendly path to Postgres)
- Add `async-trait = "0.1"` (required by graph-flow's `Task` trait)

### New Files (owned by this change)
- `src/workflow/mod.rs` — public module root, re-exports `TradingPipeline` and `SnapshotStore`
- `src/workflow/pipeline.rs` — `TradingPipeline` struct: accepts owned `Config`, data clients,
  and snapshot store; handles graph construction, session setup, `FlowRunner` execution, and the
  public `run_analysis_cycle()` entry point using existing provider helper functions from
  `src/providers/factory.rs`
- `src/workflow/tasks.rs` — 14 `Task` trait implementations wrapping individual agents:
  - `FundamentalAnalystTask`, `SentimentAnalystTask`, `NewsAnalystTask`, `TechnicalAnalystTask`
  - `AnalystSyncTask` (merges fan-out results, enforces degradation policy)
  - `BullishResearcherTask`, `BearishResearcherTask`, `DebateModeratorTask`
  - `TraderTask`
  - `AggressiveRiskTask`, `ConservativeRiskTask`, `NeutralRiskTask`
  - `RiskModeratorTask`
  - `FundManagerTask`
- `src/workflow/snapshot.rs` — SQLite-based `SnapshotStore` for phase snapshots
- `src/workflow/context_bridge.rs` — Helpers for serializing/deserializing `TradingState` to/from
  graph-flow `Context` (the "Context-as-envelope" pattern)
- `migrations/0001_create_phase_snapshots.sql` — SQLx migration creating the
  `phase_snapshots` table for SQLite-backed audit snapshots

### Graph Topology
11 task nodes connected via `GraphBuilder`:
1. **Phase 1 — Analyst Fan-Out**: `FanOutTask` containing 4 analyst child tasks → `AnalystSyncTask`
2. **Phase 2 — Researcher Debate**: `BullishResearcherTask` → `BearishResearcherTask` →
   `DebateModeratorTask` with conditional edge looping back when `debate_round < max_debate_rounds`
3. **Phase 3 — Trader Synthesis**: `TraderTask` (sequential)
4. **Phase 4 — Risk Discussion**: `AggressiveRiskTask` → `ConservativeRiskTask` →
   `NeutralRiskTask` → `RiskModeratorTask` with conditional edge looping back when
   `risk_round < max_risk_rounds`
5. **Phase 5 — Fund Manager**: `FundManagerTask` → `End`

### State Management
- **Context-as-envelope**: Full `TradingState` serialized into graph-flow `Context` under the key
  `"trading_state"`. Each task deserializes on entry, operates on typed state, and re-serializes on
  exit.
- **Fan-out coordination**: Analyst child tasks write results to prefixed Context keys
  (`"analyst.fundamental"`, etc.). `AnalystSyncTask` merges them back into `TradingState`.
- **Phase snapshots**: After each phase boundary, the current `TradingState` is persisted to SQLite
  (`phase_snapshots` table) via `sqlx`.

### Token Accounting
Each task wrapper captures `AgentTokenUsage` from agent return values. At phase boundaries,
accumulated agent usage entries are finalized into `PhaseTokenUsage` (with timing) and appended to
`TradingState.token_usage.phase_usage`. Cyclic phases follow the upstream `core-types`,
`researcher-debate`, and `risk-management` contracts by recording per-round entries plus a
moderation entry, not a single flattened entry for the whole cyclic phase.

### Observability
- Emit structured `tracing` spans/events for cycle start/end, phase transitions, per-task
  execution, debate/risk round boundaries, snapshot persistence, and failures so downstream CLI/TUI
  layers can stream pipeline progress in real time.

## Impact

- **Affected specs**: `graph-orchestration` (new)
- **Affected code**:
  - `src/workflow/mod.rs` (owned, currently empty skeleton)
  - `src/workflow/pipeline.rs` (new)
  - `src/workflow/tasks.rs` (new)
  - `src/workflow/snapshot.rs` (new)
  - `src/workflow/context_bridge.rs` (new)
  - `migrations/0001_create_phase_snapshots.sql` (new)
- **Dependencies**: ALL agent changes must be complete:
  - `add-analyst-team` (Phase 1 agents)
  - `add-researcher-debate` (Phase 2 agents)
  - `add-trader-agent` (Phase 3 agent)
  - `add-risk-management` (Phase 4 agents)
  - `add-fund-manager` (Phase 5 agent)
  - `add-llm-providers` (completion-model helpers for model tier routing)
  - `add-financial-data` (FinnhubClient, YFinanceClient for analyst tools)
  - `add-technical-analysis` (kand indicator tools for technical analyst)
- **Downstream consumers**:
  - `add-cli` (invokes `run_analysis_cycle()` from CLI subcommands)
  - `add-backtesting` (replays the pipeline over historical data)

## Cross-Owner Changes

- `Cargo.toml` — owner: `add-project-foundation`. Justification: adds `graph-flow`, `sqlx`, and
  `async-trait` as new dependencies. These were planned but not pre-declared since graph-flow was
  a deferred dependency.
- `src/workflow/mod.rs` — owner: `add-project-foundation` (skeleton). Justification: replaces the
  empty `// Populated by add-graph-orchestration` placeholder with actual module declarations. This
  is the intended use of the skeleton.
- `src/error.rs` — owner: `add-project-foundation`. Justification: adds a graph-orchestration-
  specific `TradingError::GraphFlow { phase, task, cause }` variant so `FlowRunner` failures can be
  mapped into typed domain errors with preserved task/phase context.
- `src/providers/factory.rs` — owner: `add-llm-providers`. Justification: exhaustive matches on
  `TradingError` (for retry classification and error propagation) will need the new
  `TradingError::GraphFlow { phase, task, cause }` variant handled explicitly.
