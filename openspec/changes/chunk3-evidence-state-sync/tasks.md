## 1. Typed State Modules

- [x] 1.1 Create `src/state/provenance.rs` with `EvidenceSource` and `DataQualityFlag`, both deriving serde and
  `JsonSchema`.
- [x] 1.2 Create `src/state/evidence.rs` with `EvidenceKind` and generic `EvidenceRecord<T>`.

  Important: the docs intentionally do not force one exact generic-bound syntax for derive macros. Use the smallest
  compiling serde/schemars-compatible form so `EvidenceRecord<FundamentalData>`, `EvidenceRecord<TechnicalData>`,
  `EvidenceRecord<SentimentData>`, `EvidenceRecord<NewsData>`, and `EvidenceRecord<serde_json::Value>` all work.

- [x] 1.3 Create `src/state/reporting.rs` with `DataCoverageReport` and `ProvenanceSummary`.
- [x] 1.4 Export the three modules and their public types from `src/state/mod.rs`.
- [x] 1.5 Add serde round-trip tests for each new state module.
- [x] 1.6 Run `cargo test --lib state -- --nocapture`.
- [x] 1.7 Commit: `feat: add evidence provenance and reporting state modules`.

## 2. TradingState Extension And Persistence Coverage

- [x] 2.1 Add the six additive fields to `src/state/trading_state.rs`:

  - `evidence_fundamental`
  - `evidence_technical`
  - `evidence_sentiment`
  - `evidence_news`
  - `data_coverage`
  - `provenance_summary`

- [x] 2.2 Initialize all six fields to `None` in `TradingState::new`.
- [x] 2.3 Run `cargo build` and update every `TradingState { ... }` literal that becomes incomplete.

  Likely spillover files include:

  - `src/workflow/context_bridge.rs`
  - `src/workflow/snapshot.rs`
  - `src/workflow/tasks/tests.rs`
  - `src/agents/researcher/common.rs`
  - `src/agents/researcher/bullish.rs`
  - `src/agents/researcher/bearish.rs`
  - `src/agents/researcher/moderator.rs`
  - `src/agents/risk/common.rs`
  - `src/agents/risk/moderator.rs`
  - `src/agents/trader/tests.rs`
  - `src/agents/fund_manager/tests.rs`
  - integration tests under `tests/`

  Follow compiler errors rather than stopping at this list.

- [x] 2.4 Add a context-bridge round-trip test proving the new evidence/coverage/provenance fields survive
  `serialize_state_to_context` / `deserialize_state_from_context`.
- [x] 2.5 Add a snapshot round-trip test proving the new evidence/coverage/provenance fields survive save/load.
- [x] 2.6 Run:

  - `cargo test --lib workflow::context_bridge -- --nocapture`
  - `cargo test --lib workflow::snapshot -- --nocapture`

- [x] 2.7 Commit: `feat: extend TradingState with evidence and reporting fields`.

## 3. AnalystSyncTask Dual-Write And Derivation Rules

- [x] 3.1 Add Stage 1 source-mapping helpers or constants in `src/workflow/tasks/analyst.rs`:

  - fundamentals → `finnhub` / `fundamentals`
  - sentiment → `finnhub` / `company_news_sentiment_inputs`
  - news → `finnhub` + `fred` / `company_news` + `macro_indicators`
  - technical → `yfinance` / `ohlcv`

- [x] 3.2 Update `AnalystSyncTask` so each successful merged analyst result dual-writes both the legacy field and the
  corresponding `evidence_*` field with `quality_flags: vec![]`.
- [x] 3.3 Derive `DataCoverageReport` from the typed `evidence_*` fields only, using the fixed required input order
  `["fundamentals", "sentiment", "news", "technical"]`.

  This report is materialized on the continue path only (`0-1` analyst failures). If `AnalystSyncTask` aborts because
  `2+` analysts failed, do not fabricate a partial coverage report before returning the error.

- [x] 3.4 Derive `ProvenanceSummary` from the providers attached to evidence records that are actually present on the
  continue path.

  `providers_used` must be sorted ascending and deduplicated. Do **not** pre-populate absent providers just because
  they are part of the Stage 1 source map.

- [x] 3.5 Extend `analyst_sync_all_succeed_returns_continue` in `src/workflow/tasks/tests.rs` to assert:

  - all four `evidence_*` fields are `Some`
  - `data_coverage.missing_inputs` is empty
  - `provenance_summary.providers_used == ["finnhub", "fred", "yfinance"]`

- [x] 3.6 Add a new regression test exercising the one-failure continue path, for example
  `analyst_sync_one_missing_input_marks_coverage_and_provenance`.

  Seed context so exactly three analysts succeed and the technical analyst fails. Assert:

  - `result.next_action == NextAction::Continue`
  - `state.evidence_technical.is_none()`
  - `state.data_coverage.missing_inputs == ["technical"]`
  - `state.provenance_summary.providers_used == ["finnhub", "fred"]`

- [x] 3.7 Run `cargo test --lib workflow::tasks -- --nocapture`.
- [x] 3.8 Commit: `feat: dual-write analyst evidence and coverage metadata in AnalystSyncTask`.

## 4. Shared Prompt Builders And Downstream Consumer Boundaries

- [x] 4.1 Add `build_evidence_context(state: &TradingState) -> String` to `src/agents/shared/prompt.rs`.

  Required shape:

  ```text
  Typed evidence snapshot:
  - fundamentals: <json or null>
  - sentiment: <json or null>
  - news: <json or null>
  - technical: <json or null>
  ```

  Reuse the existing shared prompt-safe serialization/sanitization helpers. Never panic.

- [x] 4.2 Add `build_data_quality_context(state: &TradingState) -> String` to `src/agents/shared/prompt.rs`.

  Required shape:

  ```text
  Data quality snapshot:
  - required_inputs: [...]
  - missing_inputs: [...]
  - providers_used: [...]
  ```

  Read from `state.data_coverage` and `state.provenance_summary`. Return a compact fallback string when both are
  `None`. If one side is present and the other absent, render the available fields and use a compact fallback or empty
  label for the missing side. Never panic.

- [x] 4.3 Add unit tests in `src/agents/shared/prompt.rs` proving both new builders contain expected keys when state is
  populated and return non-empty fallback text when the relevant fields are absent.

- [x] 4.4 In `src/agents/researcher/common.rs`, extend the existing `build_analyst_context(state)` helper so it appends
  `build_evidence_context(state)` and `build_data_quality_context(state)` after the legacy analyst snapshot. This is the
  preferred single injection point for bullish, bearish, and moderator prompt paths.

- [x] 4.5 In `src/agents/risk/common.rs`, extend the existing `build_analyst_context(state)` helper the same way. This
  is the preferred single injection point for the risk persona agents and the risk moderator.

- [x] 4.6 In `src/agents/trader/mod.rs::build_prompt_context`, append the new typed evidence and data-quality blocks at
  the existing dynamic prompt-construction site. Keep the legacy analyst report fields during the Stage 1 dual-write
  period.

- [x] 4.7 In `src/agents/fund_manager/prompt.rs::build_prompt_context` / `build_user_prompt`, append the new typed
  evidence and data-quality blocks to the serialized untrusted runtime context. Keep runtime data in the user prompt;
  do not move it into the static system prompt.

- [x] 4.8 Add string-contains tests covering the actual injection boundaries:

  - `src/agents/researcher/common.rs`: `build_analyst_context(...)` includes both new shared context blocks
  - `src/agents/risk/common.rs`: `build_analyst_context(...)` includes both new shared context blocks
  - `src/agents/trader/tests.rs`: `build_prompt_context(...)` includes typed evidence and data-quality context
  - `src/agents/fund_manager/prompt.rs`: `build_prompt_context(...)` or `build_user_prompt(...)` includes typed
    evidence and data-quality context in the user prompt

- [x] 4.9 Run:

  - `cargo test --lib agents::shared::prompt -- --nocapture`
  - `cargo test --lib agents::researcher -- --nocapture`
  - `cargo test --lib agents::risk -- --nocapture`
  - `cargo test --lib agents::trader -- --nocapture`
  - `cargo test --lib agents::fund_manager -- --nocapture`

- [x] 4.10 Record cross-owner approval and owner awareness.

  Before implementation begins, obtain maintainer or owner approval for the foreign-owned files listed in
  `proposal.md`. When the owner change's `tasks.md` exists in the OpenSpec tree, add a `### Cross-Owner Touch-points`
  note there for awareness.

- [x] 4.11 Commit: `feat: inject typed evidence and quality context into downstream prompts`.

## 5. Verification

- [x] 5.1 Run `cargo fmt -- --check`.
- [x] 5.2 Run `cargo clippy --all-targets -- -D warnings`.
- [x] 5.3 Run `cargo nextest run --all-features --locked`.
- [x] 5.4 Run `cargo build`.
- [ ] 5.5 Run `openspec validate chunk3-evidence-state-sync --strict`.
