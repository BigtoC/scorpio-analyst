# Tasks for `chunk2-config-entity-preflight`

## 0. Approval Gate

- [ ] 0.1 Obtain approval for the cross-owner file changes listed in `proposal.md` before implementation begins

## 1. Enrichment Config

- [ ] 1.1 Add `DataEnrichmentConfig` to `src/config.rs` and attach it to `Config` as `#[serde(default)] pub enrichment: DataEnrichmentConfig`
- [ ] 1.2 Add `[enrichment]` defaults to `config.toml` with `enable_transcripts = false`, `enable_consensus_estimates = false`, `enable_event_news = false`, and `max_evidence_age_hours = 48`
- [ ] 1.3 Extend `Config::validate()` to call the shared symbol validator before the existing LLM-key warning, and make the `data::symbol` module visible enough for `src/config.rs` to reuse it
- [ ] 1.4 Update config tests in `src/config.rs` for enrichment defaults/env overrides and symbol validation, and update every manual `Config { ... }` literal in the repo that compilation flags (notably `src/agents/trader/tests.rs`, `src/agents/fund_manager/tests.rs`, `tests/support/workflow_pipeline_make_pipeline.rs`, and `tests/support/workflow_observability_pipeline_support.rs`)

## 2. Entity Resolution And Adapter Contracts

- [ ] 2.1 Create `src/data/entity.rs` with `ResolvedInstrument` and `resolve_symbol`, delegating validation to `src/data/symbol.rs` and canonicalizing accepted symbols to uppercase
- [ ] 2.2 Create `src/data/adapters/mod.rs` with `ProviderCapabilities::from_config(&DataEnrichmentConfig)`
- [ ] 2.3 Create `src/data/adapters/transcripts.rs`, `src/data/adapters/estimates.rs`, and `src/data/adapters/events.rs` with the Stage 1 contract-only evidence structs and provider traits (`TranscriptEvidence` / `TranscriptProvider`, `ConsensusEvidence` / `EstimatesProvider`, `EventNewsEvidence` / `EventNewsProvider`)
- [ ] 2.4 Update `src/data/mod.rs` to export `entity` and `adapters`, and to widen `symbol` visibility just enough for shared validator reuse
- [ ] 2.5 Add focused unit tests for `resolve_symbol`, `ProviderCapabilities`, and the new adapter payload types/serde round-trips

## 3. Preflight Context Keys And Task

- [ ] 3.1 Extend the existing `src/workflow/tasks/common.rs` with `KEY_RESOLVED_INSTRUMENT`, `KEY_PROVIDER_CAPABILITIES`, `KEY_REQUIRED_COVERAGE_INPUTS`, `KEY_CACHED_TRANSCRIPT`, `KEY_CACHED_CONSENSUS`, and `KEY_CACHED_EVENT_FEED`
- [ ] 3.2 Create `src/workflow/tasks/preflight.rs` implementing the current `graph_flow::Task` API (`fn id(&self) -> &str`, `async fn run(&self, context: Context) -> graph_flow::Result<TaskResult>`)
- [ ] 3.3 In `PreflightTask`, load `TradingState` from context, resolve `state.asset_symbol`, write the canonical symbol back into `TradingState.asset_symbol`, serialize the updated state, write `ResolvedInstrument` and `ProviderCapabilities` to context, write the fixed ordered required inputs `['fundamentals', 'sentiment', 'news', 'technical']`, and seed typed JSON `null` placeholders for transcript/consensus/event-feed caches
- [ ] 3.4 Update `src/workflow/tasks/mod.rs` to export `preflight` plus the new preflight-related constants from `common`
- [ ] 3.5 Add unit tests for `PreflightTask` proving canonical symbol write-back, all required context keys, typed `null` cache placeholders, and fail-closed behavior on invalid symbol/context corruption

## 4. Pipeline Wiring

- [ ] 4.1 Update `src/workflow/pipeline.rs` to add `PreflightTask` as a new first node, add the edge `preflight -> analyst_fanout`, change `graph.set_start_task(...)` to `preflight`, and update `Session::new_from_task(...)` bootstrap to start at `preflight`
- [ ] 4.2 Update pipeline task-id tests and workflow structure tests for the new start task and increased node count, especially `tests/workflow_pipeline_structure.rs`
- [ ] 4.3 Update workflow test-support exports only if needed by integration tests that inspect preflight state or keys

## 5. Verification

- [ ] 5.1 Run `cargo fmt -- --check`
- [ ] 5.2 Run `cargo clippy --all-targets -- -D warnings`
- [ ] 5.3 Run `cargo test`
- [ ] 5.4 Run `cargo build`
- [ ] 5.5 Run `openspec validate chunk2-config-entity-preflight --strict`
