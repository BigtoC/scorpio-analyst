# Tasks for `add-graph-orchestration`

## Prerequisites

- [ ] `add-project-foundation` is complete (TradingState, core types, config with `max_debate_rounds` and
      `max_risk_rounds`, error handling, module stubs including `src/workflow/mod.rs`)
- [ ] `add-llm-providers` is complete (completion-model helpers, model tier routing, retry-wrapped helpers)
- [ ] `add-analyst-team` is complete (4 analysts, `run_analyst_team`, fan-out with degradation policy)
- [ ] `add-researcher-debate` is complete (`run_researcher_debate`, cyclic debate loop)
- [ ] `add-trader-agent` is complete (`run_trader`, `TradeProposal` generation)
- [ ] `add-risk-management` is complete (`run_risk_discussion`, cyclic risk loop)
- [ ] `add-fund-manager` is complete (`run_fund_manager`, final approve/reject decision)
- [ ] `add-financial-data` is complete (`FinnhubClient`, `YFinanceClient`, rig tool wrappers)
- [ ] `add-technical-analysis` is complete (kand indicator tools and calculator)

## 1. Dependencies (`Cargo.toml`)

- [ ] 1.1 Add `graph-flow` from the forked `BigtoC/rs-graph-llm` branch `feature/update-rig-version`, enabling the
      `rig` feature with `rig-core 0.32`
- [ ] 1.2 Add `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }`
- [ ] 1.3 Add `async-trait = "0.1"`
- [ ] 1.4 Verify `cargo build` compiles cleanly with the new dependencies

## 2. Context Bridge (`src/workflow/context_bridge.rs`)

- [ ] 2.1 Implement `serialize_state_to_context(state: &TradingState, context: &Context) -> Result<()>`
      that serializes the full `TradingState` as JSON under the key `"trading_state"`
- [ ] 2.2 Implement `deserialize_state_from_context(context: &Context) -> Result<TradingState>` that
      reads the `"trading_state"` key and deserializes back into `TradingState`
- [ ] 2.3 Implement fan-out prefix helpers:
      `write_prefixed_result<T: Serialize>(context: &Context, prefix: &str, key: &str, value: &T) -> Result<()>`
      and `read_prefixed_result<T: DeserializeOwned>(context: &Context, prefix: &str, key: &str) -> Result<T>`
- [ ] 2.4 Write unit tests for round-trip serialization of `TradingState` through Context
- [ ] 2.5 Write unit tests for missing key handling returning appropriate `TradingError`
- [ ] 2.6 Write unit tests for prefixed read/write with multiple analyst results

## 3. Snapshot Store (`src/workflow/snapshot.rs`)

- [ ] 3.1 Define `SnapshotStore` struct wrapping `sqlx::SqlitePool`
- [ ] 3.2 Implement `SnapshotStore::new(db_url: &str) -> Result<Self>` that creates the pool and runs
      the initial migration
- [ ] 3.3 Add `migrations/0001_create_phase_snapshots.sql` defining the `phase_snapshots` table for SQLx
- [ ] 3.4 Define the SQLite migration: `CREATE TABLE phase_snapshots` with columns `execution_id TEXT`,
      `phase_number INTEGER`, `phase_name TEXT`, `trading_state_json TEXT`, `token_usage_json TEXT`,
      `created_at TEXT`, and a `UNIQUE(execution_id, phase_number)` constraint
- [ ] 3.5 Implement `save_snapshot(&self, execution_id: &str, phase_number: u8, phase_name: &str,
      state: &TradingState, token_usage: Option<&[AgentTokenUsage]>) -> Result<()>`
- [ ] 3.6 Implement `load_snapshot(&self, execution_id: &str, phase_number: u8) -> Result<Option<(TradingState, Option<Vec<AgentTokenUsage>>)>>`
- [ ] 3.7 Write unit tests with in-memory SQLite (`sqlite::memory:`) for save/load round-trip
- [ ] 3.8 Write unit tests verifying duplicate phase insertion uses upsert semantics
- [ ] 3.9 Write unit tests verifying missing snapshot returns `None`

## 4. Task Wrappers — Phase 1 Analyst Fan-Out (`src/workflow/tasks.rs`)

- [ ] 4.1 Implement `FundamentalAnalystTask` struct implementing graph-flow `Task` trait:
      `fn id(&self) -> &str` returns `"fundamental_analyst"`;
      `async fn run(&self, context: Context) -> Result<TaskResult>` deserializes `TradingState`, invokes
      `FundamentalAnalyst`, writes result to prefixed Context key `"analyst.fundamental"`, returns `Continue`
- [ ] 4.2 Implement `SentimentAnalystTask` analogously with id `"sentiment_analyst"` and prefix
      `"analyst.sentiment"`
- [ ] 4.3 Implement `NewsAnalystTask` analogously with id `"news_analyst"` and prefix `"analyst.news"`
- [ ] 4.4 Implement `TechnicalAnalystTask` analogously with id `"technical_analyst"` and prefix
      `"analyst.technical"`
- [ ] 4.5 Implement `AnalystSyncTask` that reads all 4 prefixed analyst results, merges into `TradingState`,
      enforces degradation policy (1 fail = continue with partial data, 2+ fails = abort with `End`),
      saves phase snapshot via `SnapshotStore`, returns `Continue` or `End`
- [ ] 4.6 Write unit tests for each analyst task wrapper (mock agent, verify Context writes)
- [ ] 4.7 Write unit tests for `AnalystSyncTask`: all 4 succeed, 1 fails (continues), 2 fail (aborts)

## 5. Task Wrappers — Phase 2 Researcher Debate (`src/workflow/tasks.rs`)

- [ ] 5.1 Implement `BullishResearcherTask` implementing `Task` trait with id `"bullish_researcher"`;
      increments `"debate_round"` counter in Context on each invocation
- [ ] 5.2 Implement `BearishResearcherTask` implementing `Task` trait with id `"bearish_researcher"`
- [ ] 5.3 Implement `DebateModeratorTask` implementing `Task` trait with id `"debate_moderator"`;
      saves a phase snapshot on its final invocation (when debate is complete);
      returns `Continue` (graph conditional edge handles cycling vs. advancing)
- [ ] 5.4 Implement condition function for debate loop:
      `|context| context.get::<u32>("debate_round") < context.get::<u32>("max_debate_rounds")`
- [ ] 5.5 Write unit tests for researcher task wrappers verifying Context state mutations
- [ ] 5.6 Write unit tests for debate round counter increment across multiple invocations

## 6. Task Wrappers — Phase 3 Trader (`src/workflow/tasks.rs`)

- [ ] 6.1 Implement `TraderTask` implementing `Task` trait with id `"trader"`;
      deserializes `TradingState`, invokes `run_trader`, re-serializes updated state, saves phase
      snapshot, returns `Continue`
- [ ] 6.2 Write unit tests for `TraderTask` wrapper verifying state round-trip and snapshot save

## 7. Task Wrappers — Phase 4 Risk Discussion (`src/workflow/tasks.rs`)

Phase 4 executes risk agents **sequentially** within each round (Aggressive → Conservative →
Neutral → Moderator) because each agent's prompt references the other agents' latest views
from the same round. This is NOT a fan-out.

- [ ] 7.1 Implement `AggressiveRiskTask` implementing `Task` trait with id `"aggressive_risk"`;
      increments `"risk_round"` counter in Context on each invocation, returns `Continue`
- [ ] 7.2 Implement `ConservativeRiskTask` implementing `Task` trait with id `"conservative_risk"`;
      follows `AggressiveRiskTask` sequentially, returns `Continue`
- [ ] 7.3 Implement `NeutralRiskTask` implementing `Task` trait with id `"neutral_risk"`;
      follows `ConservativeRiskTask` sequentially, returns `Continue`
- [ ] 7.4 Implement `RiskModeratorTask` implementing `Task` trait with id `"risk_moderator"`;
      follows `NeutralRiskTask` sequentially; saves a phase snapshot on its final invocation
      (when risk discussion is complete); returns `Continue` (graph conditional edge handles
      cycling vs. advancing)
- [ ] 7.5 Implement condition function for risk loop:
      `|context| context.get::<u32>("risk_round") < context.get::<u32>("max_risk_rounds")`
      — conditional edge from `RiskModeratorTask` loops back to `AggressiveRiskTask` when true,
      else advances to `FundManagerTask`
- [ ] 7.6 Write unit tests for risk task wrappers verifying Context state mutations
- [ ] 7.7 Write unit tests for risk round counter increment across multiple invocations
- [ ] 7.8 Write unit tests verifying sequential execution order: Aggressive → Conservative →
      Neutral → Moderator within a single round

## 8. Task Wrappers — Phase 5 Fund Manager (`src/workflow/tasks.rs`)

- [ ] 8.1 Implement `FundManagerTask` implementing `Task` trait with id `"fund_manager"`;
      deserializes `TradingState`, invokes `run_fund_manager`, re-serializes updated state, saves
      final phase snapshot, returns `End` (terminal node)
- [ ] 8.2 Write unit tests for `FundManagerTask` wrapper verifying terminal `End` result and snapshot save

## 9. Pipeline Construction (`src/workflow/pipeline.rs`)

- [ ] 9.1 Define `TradingPipeline` struct holding owned `Config`, `FinnhubClient`, `YFinanceClient`, and `SnapshotStore`
- [ ] 9.2 Implement `TradingPipeline::new(config: Config, finnhub: FinnhubClient, yfinance: YFinanceClient, snapshot_store: SnapshotStore) -> Result<Self>`
- [ ] 9.3 Implement `build_graph(&self) -> Graph` using `GraphBuilder` to wire all 5 phases:
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
- [ ] 9.4 Implement `run_analysis_cycle(&self, state: TradingState) -> Result<TradingState>` that
      creates `InMemorySessionStorage`, seeds `Context` with serialized `TradingState` via
      `serialize_state_to_context`, seeds `max_debate_rounds` and `max_risk_rounds` from `Config`
      into Context under their respective keys so conditional edge functions can read them, runs
      `FlowRunner`, extracts and returns final `TradingState`
- [ ] 9.5 Add structured `tracing` spans/events for cycle start/end, phase transitions, task execution,
      debate/risk rounds, snapshot persistence, and mapped failures
- [ ] 9.6 Write unit test: `build_graph` completes without error
- [ ] 9.7 Write unit test: `TradingPipeline::new` construction with mock config and clients

## 10. Module Exports (`src/workflow/mod.rs`)

- [ ] 10.1 Replace the empty skeleton with module declarations: `pub mod pipeline;`, `pub mod tasks;`,
      `pub mod snapshot;`, `pub mod context_bridge;`
- [ ] 10.2 Re-export `TradingPipeline` and `SnapshotStore` from the module root

## 11. Integration Tests

- [ ] 11.1 Write integration test: full pipeline with mocked agents — verify all 5 phases execute in order
- [ ] 11.2 Write integration test: analyst degradation — 1 mock analyst fails, pipeline continues
- [ ] 11.3 Write integration test: analyst degradation — 2 mock analysts fail, pipeline aborts after Phase 1
- [ ] 11.4 Write integration test: debate cycling — verify conditional edge loops for the configured
      number of `max_debate_rounds`
- [ ] 11.5 Write integration test: risk cycling — verify sequential execution order
      (Aggressive → Conservative → Neutral → Moderator) for the configured number of
      `max_risk_rounds`
- [ ] 11.6 Write integration test: phase snapshots — verify 5 snapshots written to SQLite after full cycle
- [ ] 11.7 Write integration test: token usage — verify `AgentTokenUsage` entries accumulated correctly
      across all phases
- [ ] 11.8 Implement error mapping from `graph-flow` errors (e.g., `GraphError`, `TaskError`) to
      `TradingError` variants so that pipeline callers receive typed errors; add a
      `TradingError::GraphFlow { phase: String, task: String, cause: String }` variant (if one
      does not already exist) that preserves structured context per the spec requirement
- [ ] 11.9 Write unit tests for error propagation: graph-flow errors are correctly mapped to
      `TradingError` and propagated from `run_analysis_cycle`
- [ ] 11.10 Write integration test: tracing emits phase/task/round transition events usable by downstream
      CLI/TUI streaming

## 12. Cross-Owner Changes

- [ ] 12.1 Modify `Cargo.toml` (owned by `add-project-foundation`): add `graph-flow`, `sqlx`, and
      `async-trait` dependencies
- [ ] 12.2 Modify `src/workflow/mod.rs` (owned by `add-project-foundation`): replace empty skeleton
      with module declarations and re-exports
- [ ] 12.3 Modify `src/error.rs` (owned by `add-project-foundation`): add
      `TradingError::GraphFlow { phase: String, task: String, cause: String }`
- [ ] 12.4 Modify `src/providers/factory.rs` (owned by `add-llm-providers`): handle
      `TradingError::GraphFlow { .. }` in exhaustive retry/error matching logic

## 13. Documentation and CI

- [ ] 13.1 Add inline doc comments (`///`) for all public types and functions in `pipeline.rs`,
      `tasks.rs`, `snapshot.rs`, and `context_bridge.rs`
- [ ] 13.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [ ] 13.3 Ensure `cargo fmt -- --check` passes
- [ ] 13.4 Ensure `cargo test` passes all new and existing tests
- [ ] 13.5 Ensure `cargo build` compiles cleanly

## 14. Verification

- [ ] 14.1 Run full `cargo test` suite and confirm zero failures
- [ ] 14.2 Run `cargo clippy -- -D warnings` and confirm zero warnings
- [ ] 14.3 Run `cargo fmt -- --check` and confirm no formatting diffs
- [ ] 14.4 Run `openspec validate add-graph-orchestration --strict` and confirm the change remains valid
- [ ] 14.5 Verify all 14 sections above are complete with every task checked off
