# Tasks for `add-graph-orchestration`

## Prerequisites

- [x] `add-project-foundation` is complete (TradingState, core types, config with `max_debate_rounds` and
      `max_risk_rounds`, error handling, module stubs including `src/workflow/mod.rs`)
- [x] `add-llm-providers` is complete (completion-model helpers, model tier routing, retry-wrapped helpers)
- [x] `add-analyst-team` is complete (4 analysts, `run_analyst_team`, fan-out with degradation policy)
- [x] `add-researcher-debate` is complete (`run_researcher_debate`, cyclic debate loop)
- [x] `add-trader-agent` is complete (`run_trader`, `TradeProposal` generation)
- [x] `add-risk-management` is complete (`run_risk_discussion`, cyclic risk loop)
- [x] `add-fund-manager` is complete (`run_fund_manager`, final approve/reject decision)
- [x] `add-financial-data` is complete (`FinnhubClient`, `YFinanceClient`, rig tool wrappers)
- [x] `add-technical-analysis` is complete (kand indicator tools and calculator)

## 1. Dependencies (`Cargo.toml`)

- [x] 1.1 Add `graph-flow` from the forked `BigtoC/rs-graph-llm` branch `feature/update-rig-version`, enabling the
      `rig` feature with `rig-core 0.32`
- [x] 1.2 Add `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }`
- [x] 1.3 Add `async-trait = "0.1"`
- [x] 1.4 Verify `cargo build` compiles cleanly with the new dependencies

## 2. Context Bridge (`src/workflow/context_bridge.rs`)

- [x] 2.1 Implement `serialize_state_to_context(state: &TradingState, context: &Context) -> Result<()>`
      that serializes the full `TradingState` as JSON under the key `"trading_state"`
- [x] 2.2 Implement `deserialize_state_from_context(context: &Context) -> Result<TradingState>` that
      reads the `"trading_state"` key and deserializes back into `TradingState`
- [x] 2.3 Implement fan-out prefix helpers:
      `write_prefixed_result<T: Serialize>(context: &Context, prefix: &str, key: &str, value: &T) -> Result<()>`
      and `read_prefixed_result<T: DeserializeOwned>(context: &Context, prefix: &str, key: &str) -> Result<T>`
- [x] 2.4 Write unit tests for round-trip serialization of `TradingState` through Context
- [x] 2.5 Write unit tests for missing key handling returning appropriate `TradingError`
- [x] 2.6 Write unit tests for prefixed read/write with multiple analyst results

## 3. Snapshot Store (`src/workflow/snapshot.rs`)

- [x] 3.1 Define `SnapshotStore` struct wrapping `sqlx::SqlitePool`
- [x] 3.2 Implement configurable SQLite path resolution for `SnapshotStore`, defaulting to
      `$HOME/.scorpio-analyst/phase_snapshots.db` when no explicit path is provided
- [x] 3.3 Ensure `SnapshotStore` creates the `$HOME/.scorpio-analyst` directory when it does not already
      exist before opening the SQLite file
- [x] 3.4 Implement `SnapshotStore::new(db_path: Option<&Path>) -> Result<Self>` (or equivalent API)
      that resolves the final SQLite path, creates the pool, and runs the initial migration
- [x] 3.5 Add `migrations/0001_create_phase_snapshots.sql` defining the `phase_snapshots` table for SQLx
- [x] 3.6 Define the SQLite migration: `CREATE TABLE phase_snapshots` with columns `execution_id TEXT`,
      `phase_number INTEGER`, `phase_name TEXT`, `trading_state_json TEXT`, `token_usage_json TEXT`,
      `created_at TEXT`, and a `UNIQUE(execution_id, phase_number)` constraint
- [x] 3.7 Implement `save_snapshot(&self, execution_id: &str, phase_number: u8, phase_name: &str,
      state: &TradingState, token_usage: Option<&[AgentTokenUsage]>) -> Result<()>`
- [x] 3.8 Implement `load_snapshot(&self, execution_id: &str, phase_number: u8) -> Result<Option<(TradingState, Option<Vec<AgentTokenUsage>>)>>`
- [x] 3.9 Write unit tests with in-memory SQLite (`sqlite::memory:`) for save/load round-trip
- [x] 3.10 Write unit tests verifying duplicate phase insertion uses upsert semantics
- [x] 3.11 Write unit tests verifying missing snapshot returns `None`
- [x] 3.12 Write unit tests verifying the default path resolves to
      `$HOME/.scorpio-analyst/phase_snapshots.db`
- [x] 3.13 Write unit tests verifying the parent directory is created automatically when absent
- [x] 3.14 Write unit tests verifying an explicit custom SQLite path overrides the default

## 4. Task Wrappers — Phase 1 Analyst Fan-Out (`src/workflow/tasks.rs`)

- [x] 4.1 Implement `FundamentalAnalystTask` struct implementing graph-flow `Task` trait:
      `fn id(&self) -> &str` returns `"fundamental_analyst"`;
      `async fn run(&self, context: Context) -> Result<TaskResult>` deserializes `TradingState`, invokes
      `FundamentalAnalyst`, writes result to prefixed Context key `"analyst.fundamental"`, returns `Continue`
- [x] 4.2 Implement `SentimentAnalystTask` analogously with id `"sentiment_analyst"` and prefix
      `"analyst.sentiment"`
- [x] 4.3 Implement `NewsAnalystTask` analogously with id `"news_analyst"` and prefix `"analyst.news"`
- [x] 4.4 Implement `TechnicalAnalystTask` analogously with id `"technical_analyst"` and prefix
      `"analyst.technical"`
- [x] 4.5 Implement `AnalystSyncTask` that reads all 4 prefixed analyst results, merges into `TradingState`,
      enforces degradation policy (1 fail = continue with partial data, 2+ fails = abort with `End`),
      saves phase snapshot via `SnapshotStore`, returns `Continue` or `End`
- [x] 4.6 Write unit tests for each analyst task wrapper (mock agent, verify Context writes)
- [x] 4.7 Write unit tests for `AnalystSyncTask`: all 4 succeed, 1 fails (continues), 2 fail (aborts)

## 5. Task Wrappers — Phase 2 Researcher Debate (`src/workflow/tasks.rs`)

- [x] 5.1 Implement `BullishResearcherTask` implementing `Task` trait with id `"bullish_researcher"`;
      increments `"debate_round"` counter in Context on each invocation
- [x] 5.2 Implement `BearishResearcherTask` implementing `Task` trait with id `"bearish_researcher"`
- [x] 5.3 Implement `DebateModeratorTask` implementing `Task` trait with id `"debate_moderator"`;
      saves a phase snapshot on its final invocation (when debate is complete);
      returns `Continue` (graph conditional edge handles cycling vs. advancing)
- [x] 5.4 Implement condition function for debate loop:
      `|context| context.get::<u32>("debate_round") < context.get::<u32>("max_debate_rounds")`
- [x] 5.5 Write unit tests for researcher task wrappers verifying Context state mutations
- [x] 5.6 Write unit tests for debate round counter increment across multiple invocations

## 6. Task Wrappers — Phase 3 Trader (`src/workflow/tasks.rs`)

- [x] 6.1 Implement `TraderTask` implementing `Task` trait with id `"trader"`;
      deserializes `TradingState`, invokes `run_trader`, re-serializes updated state, saves phase
      snapshot, returns `Continue`
- [x] 6.2 Write unit tests for `TraderTask` wrapper verifying state round-trip and snapshot save

## 7. Task Wrappers — Phase 4 Risk Discussion (`src/workflow/tasks.rs`)

Phase 4 executes risk agents **sequentially** within each round (Aggressive → Conservative →
Neutral → Moderator) because each agent's prompt references the other agents' latest views
from the same round. This is NOT a fan-out.

- [x] 7.1 Implement `AggressiveRiskTask` implementing `Task` trait with id `"aggressive_risk"`;
      increments `"risk_round"` counter in Context on each invocation, returns `Continue`
- [x] 7.2 Implement `ConservativeRiskTask` implementing `Task` trait with id `"conservative_risk"`;
      follows `AggressiveRiskTask` sequentially, returns `Continue`
- [x] 7.3 Implement `NeutralRiskTask` implementing `Task` trait with id `"neutral_risk"`;
      follows `ConservativeRiskTask` sequentially, returns `Continue`
- [x] 7.4 Implement `RiskModeratorTask` implementing `Task` trait with id `"risk_moderator"`;
      follows `NeutralRiskTask` sequentially; saves a phase snapshot on its final invocation
      (when risk discussion is complete); returns `Continue` (graph conditional edge handles
      cycling vs. advancing)
- [x] 7.5 Implement condition function for risk loop:
      `|context| context.get::<u32>("risk_round") < context.get::<u32>("max_risk_rounds")`
      — conditional edge from `RiskModeratorTask` loops back to `AggressiveRiskTask` when true,
      else advances to `FundManagerTask`
- [x] 7.6 Write unit tests for risk task wrappers verifying Context state mutations
- [x] 7.7 Write unit tests for risk round counter increment across multiple invocations
- [x] 7.8 Write unit tests verifying sequential execution order: Aggressive → Conservative →
      Neutral → Moderator within a single round

## 8. Task Wrappers — Phase 5 Fund Manager (`src/workflow/tasks.rs`)

- [x] 8.1 Implement `FundManagerTask` implementing `Task` trait with id `"fund_manager"`;
      deserializes `TradingState`, invokes `run_fund_manager`, re-serializes updated state, saves
      final phase snapshot, returns `End` (terminal node)
- [x] 8.2 Write unit tests for `FundManagerTask` wrapper verifying terminal `End` result and snapshot save

## 9. Pipeline Construction (`src/workflow/pipeline.rs`)

- [x] 9.1 Define `TradingPipeline` struct holding owned `Config`, `FinnhubClient`, `YFinanceClient`, and `SnapshotStore`
- [x] 9.2 Implement `TradingPipeline::new(config: Config, finnhub: FinnhubClient, yfinance: YFinanceClient, snapshot_store: SnapshotStore) -> Result<Self>`
- [x] 9.3 Implement `build_graph(&self) -> Graph` using `GraphBuilder` to wire all 5 phases:
      create `FanOutTask` with 4 analyst child tasks; add `AnalystSyncTask`; add conditional edge
      from `AnalystSyncTask` — if `max_debate_rounds > 0` go to `BullishResearcherTask`, else go
      to `DebateModeratorTask` (so the moderator still produces a consensus from analyst data
      alone); add `BullishResearcherTask`, `BearishResearcherTask`, `DebateModeratorTask`
      with conditional edge from moderator (loop back to `BullishResearcherTask` or advance to
      `TraderTask` based on debate round); add `TraderTask`; add conditional edge from `TraderTask`
      — if `max_risk_rounds > 0` go to `AggressiveRiskTask`, else go to `RiskModeratorTask` (so
      the moderator still produces a synthesis from the trade proposal alone); wire
      `AggressiveRiskTask` → `ConservativeRiskTask` → `NeutralRiskTask` → `RiskModeratorTask`
      sequentially; add conditional edge from `RiskModeratorTask` (loop back to
      `AggressiveRiskTask` or advance to `FundManagerTask` based on risk round); add
      `FundManagerTask`; set start task and build
- [x] 9.4 Implement `run_analysis_cycle(&self, state: TradingState) -> Result<TradingState>` that
      creates `InMemorySessionStorage`, seeds `Context` with serialized `TradingState` via
      `serialize_state_to_context`, seeds `max_debate_rounds` and `max_risk_rounds` from `Config`
      into Context under their respective keys so conditional edge functions can read them, runs
      `FlowRunner`, extracts and returns final `TradingState`
- [x] 9.5 Add structured `tracing` spans/events for cycle start/end, phase transitions, task execution,
      debate/risk rounds, snapshot persistence, and mapped failures
- [x] 9.6 Write unit test: `build_graph` completes without error
- [x] 9.7 Write unit test: `TradingPipeline::new` construction with mock config and clients

## 10. Module Exports (`src/workflow/mod.rs`)

- [x] 10.1 Replace the empty skeleton with module declarations: `pub mod pipeline;`, `pub mod tasks;`,
      `pub mod snapshot;`, `pub mod context_bridge;`
- [x] 10.2 Re-export `TradingPipeline` and `SnapshotStore` from the module root

## 11. Integration Tests

- [x] 11.1 Write integration test: full pipeline with mocked agents — verify all 5 phases execute in order
- [x] 11.2 Write integration test: analyst degradation — 1 mock analyst fails, pipeline continues
- [x] 11.3 Write integration test: analyst degradation — 2 mock analysts fail, pipeline aborts after Phase 1
- [x] 11.4 Write integration test: debate cycling — verify conditional edge loops for the configured
      number of `max_debate_rounds`
- [x] 11.5 Write integration test: risk cycling — verify sequential execution order
      (Aggressive → Conservative → Neutral → Moderator) for the configured number of
      `max_risk_rounds`
- [x] 11.6 Write integration test: phase snapshots — verify 5 snapshots written to SQLite after full cycle
- [x] 11.7 Write integration test: token usage — verify `AgentTokenUsage` entries accumulated correctly
      across all phases
- [x] 11.8 Implement error mapping from `graph-flow` errors (e.g., `GraphError`, `TaskError`) to
      `TradingError` variants so that pipeline callers receive typed errors; add a
      `TradingError::GraphFlow { phase: String, task: String, cause: String }` variant (if one
      does not already exist) that preserves structured context per the spec requirement
- [x] 11.9 Write unit tests for error propagation: graph-flow errors are correctly mapped to
      `TradingError` and propagated from `run_analysis_cycle`
- [x] 11.10 Write integration test: tracing emits phase/task/round transition events usable by downstream
      CLI/TUI streaming

## 11-R. Remediation: Real Per-Node Execution

_These tasks were not in the original implementation plan. They are required to bring the
implementation into full spec-compliance. All tasks below are blocked on cross-owner approval
for `src/agents/researcher/mod.rs`, `src/agents/risk/mod.rs`, and `src/agents/analyst/mod.rs`._

- [x] R-1 Add single-step researcher public helpers to `src/agents/researcher/mod.rs`:
      `run_bullish_researcher_turn`, `run_bearish_researcher_turn`, `run_debate_moderation`
- [x] R-2 Add single-step risk public helpers to `src/agents/risk/mod.rs`:
      `run_aggressive_risk_turn`, `run_conservative_risk_turn`, `run_neutral_risk_turn`,
      `run_risk_moderation`
- [x] R-3 Add shared cached-news prefetch helper to `src/agents/analyst/mod.rs`:
      `fetch_shared_news` (or equivalent)
- [x] R-4 Refactor `BullishResearcherTask` to call `run_bullish_researcher_turn` (not the full loop)
- [x] R-5 Refactor `BearishResearcherTask` from a no-op to call `run_bearish_researcher_turn`
- [x] R-6 Refactor `DebateModeratorTask` to call `run_debate_moderation` and increment `debate_round`
      at the moderator checkpoint
- [x] R-7 Refactor `AggressiveRiskTask` to call `run_aggressive_risk_turn` (not the full loop);
      do not increment `risk_round` here
- [x] R-8 Refactor `ConservativeRiskTask` from a no-op to call `run_conservative_risk_turn`
- [x] R-9 Refactor `NeutralRiskTask` from a no-op to call `run_neutral_risk_turn`
- [x] R-10 Refactor `RiskModeratorTask` to call `run_risk_moderation` and increment `risk_round`
       at the moderator checkpoint
- [x] R-11 Make `run_analysis_cycle` generate a fresh `Uuid` and write it to
       `TradingState.execution_id` before the graph starts (overwriting caller-supplied IDs)
- [x] R-12 Make snapshot persistence failures fatal in all workflow tasks (remove log-and-continue)
- [x] R-13 Switch `SnapshotStore` schema initialization to `sqlx::migrate!` (migration-driven)
- [x] R-14 Write per-phase `PhaseTokenUsage` entries into `TradingState.token_usage` at phase
       boundaries (analyst, per-debate-round, debate-moderation, trader, per-risk-round,
       risk-moderation, fund-manager)
- [x] R-15 Fix `TradingError::GraphFlow` mapping to carry real phase/task identity (not `step_N`)
- [x] R-16 Add tests for zero-round debate routing to `DebateModeratorTask` with real moderator call
- [x] R-17 Add tests for zero-round risk routing to `RiskModeratorTask` with real moderator call
- [x] R-18 Add tests verifying snapshot failure causes task error propagation

## 12. Cross-Owner Changes

- [x] 12.1 Modify `Cargo.toml` (owned by `add-project-foundation`): add `graph-flow`, `sqlx`, and
      `async-trait` dependencies
- [x] 12.2 Modify `src/workflow/mod.rs` (owned by `add-project-foundation`): replace empty skeleton
      with module declarations and re-exports
- [x] 12.3 Modify `src/error.rs` (owned by `add-project-foundation`): add
      `TradingError::GraphFlow { phase: String, task: String, cause: String }`
- [x] 12.4 Modify `src/providers/factory.rs` (owned by `add-llm-providers`): handle
      `TradingError::GraphFlow { .. }` in exhaustive retry/error matching logic

## 13. Documentation and CI

- [x] 13.1 Add inline doc comments (`///`) for all public types and functions in `pipeline.rs`,
      `tasks.rs`, `snapshot.rs`, and `context_bridge.rs`
- [x] 13.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [x] 13.3 Ensure `cargo fmt -- --check` passes
- [x] 13.4 Ensure `cargo test` passes all new and existing tests
- [x] 13.5 Ensure `cargo build` compiles cleanly

## 14. Verification

- [x] 14.1 Run full `cargo test` suite and confirm zero failures
- [x] 14.2 Run `cargo clippy -- -D warnings` and confirm zero warnings
- [x] 14.3 Run `cargo fmt -- --check` and confirm no formatting diffs
- [x] 14.4 Run `openspec validate add-graph-orchestration --strict` and confirm the change remains valid
- [x] 14.5 Verify all 14 sections above are complete with every task checked off
- [x] 14.6 After remediation: re-run full verification suite and confirm all new and original tests pass

### Cross-Owner Touch-points

- Approved for `chunk3-evidence-state-sync` to update `src/workflow/context_bridge.rs`, `src/workflow/snapshot.rs`,
  `src/workflow/tasks/analyst.rs`, and `src/workflow/tasks/tests.rs` to persist/round-trip the new typed evidence
  fields and derive run-level coverage/provenance metadata on the analyst sync continue path.
