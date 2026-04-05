## 1. New State Modules

- [ ] 1.1 Create `src/state/provenance.rs` with exactly:

  ```rust
  use serde::{Deserialize, Serialize};
  use schemars::JsonSchema;

  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
  pub struct EvidenceSource {
      pub provider: String,
      pub dataset: String,
      pub fetched_at: String,
      pub effective_at: Option<String>,
      pub symbol: Option<String>,
      pub url: Option<String>,
      pub citation: Option<String>,
      pub freshness_hours: Option<u64>,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
  pub enum DataQualityFlag {
      Missing,
      Stale,
      Partial,
      Estimated,
      Conflicted,
      LowConfidence,
  }
  ```

  Add a `#[cfg(test)]` module with at least one serde round-trip test:
  - `evidence_source_roundtrip`: construct an `EvidenceSource`, serialize to JSON, deserialize, assert equality.
  - `data_quality_flag_roundtrip`: serialize `DataQualityFlag::Stale`, deserialize, assert equality.

- [ ] 1.2 Create `src/state/evidence.rs` with exactly:

  ```rust
  use serde::{Deserialize, Serialize};
  use schemars::JsonSchema;
  use super::provenance::{EvidenceSource, DataQualityFlag};

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
  pub enum EvidenceKind {
      Fundamental,
      Technical,
      Sentiment,
      News,
      Macro,
      Transcript,
      Estimates,
      Peers,
      Volatility,
  }

  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
  pub struct EvidenceRecord<T: Serialize + for<'de> Deserialize<'de> + JsonSchema> {
      pub kind: EvidenceKind,
      pub payload: T,
      pub sources: Vec<EvidenceSource>,
      pub quality_flags: Vec<DataQualityFlag>,
  }
  ```

  Add a `#[cfg(test)]` module with at least:
  - `evidence_kind_roundtrip`: serialize `EvidenceKind::Fundamental`, deserialize, assert equality.
  - `evidence_record_roundtrip`: construct an `EvidenceRecord<serde_json::Value>` with a JSON payload,
    serialize to JSON, deserialize, assert `kind` and `sources` are preserved.

- [ ] 1.3 Create `src/state/reporting.rs` with exactly:

  ```rust
  use serde::{Deserialize, Serialize};
  use schemars::JsonSchema;

  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
  pub struct DataCoverageReport {
      pub required_inputs: Vec<String>,
      pub missing_inputs: Vec<String>,
      pub stale_inputs: Vec<String>,
      pub partial_inputs: Vec<String>,
  }

  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
  pub struct ProvenanceSummary {
      pub providers_used: Vec<String>,
      pub generated_at: String,
      pub caveats: Vec<String>,
  }
  ```

  Add a `#[cfg(test)]` module with at least:
  - `data_coverage_report_roundtrip`: construct a `DataCoverageReport` with non-empty vecs, round-trip.
  - `provenance_summary_roundtrip`: construct a `ProvenanceSummary`, round-trip.

- [ ] 1.4 Export the three new modules from `src/state/mod.rs`:

  ```rust
  pub mod evidence;
  pub mod provenance;
  pub mod reporting;
  pub use evidence::{EvidenceKind, EvidenceRecord};
  pub use provenance::{EvidenceSource, DataQualityFlag};
  pub use reporting::{DataCoverageReport, ProvenanceSummary};
  ```

- [ ] 1.5 Run `cargo test --lib state -- --nocapture` and confirm PASS.
- [ ] 1.6 Commit: `git add src/state/evidence.rs src/state/provenance.rs src/state/reporting.rs src/state/mod.rs && git commit -m "feat: add evidence provenance and reporting state modules"`

## 2. TradingState Extension and Round-Trip Tests

- [ ] 2.1 Import the new types at the top of `src/state/trading_state.rs`:

  ```rust
  use crate::state::{
      evidence::{EvidenceKind, EvidenceRecord},
      reporting::{DataCoverageReport, ProvenanceSummary},
  };
  use crate::state::{FundamentalData, TechnicalData, SentimentData, NewsData};
  ```

  Verify that `FundamentalData`, `TechnicalData`, `SentimentData`, and `NewsData` derive `JsonSchema`.
  If any are missing `JsonSchema`, add `#[derive(JsonSchema)]` to those types before proceeding.

- [ ] 2.2 Add the six new fields to `TradingState`:

  ```rust
  pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
  pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
  pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
  pub evidence_news: Option<EvidenceRecord<NewsData>>,
  pub data_coverage: Option<DataCoverageReport>,
  pub provenance_summary: Option<ProvenanceSummary>,
  ```

  Initialize all six to `None` in `TradingState::new`.

- [ ] 2.3 Run `cargo build` and update every `TradingState { ... }` literal that `cargo build` flags as
  incomplete. Add `evidence_fundamental: None, evidence_technical: None, evidence_sentiment: None,
  evidence_news: None, data_coverage: None, provenance_summary: None` to each.

  The most likely spillover files are:
  - `src/workflow/context_bridge.rs`
  - `src/workflow/snapshot.rs`
  - `src/workflow/tasks/tests.rs`
  - `src/agents/researcher/common.rs`
  - `src/agents/risk/common.rs`
  - Integration test files under `tests/`

  Do not stop at this list — follow all compiler errors.

- [ ] 2.4 Add a round-trip test to `src/workflow/context_bridge.rs`:

  ```rust
  #[test]
  fn trading_state_with_evidence_fields_roundtrips_through_context() {
      // Populate evidence_fundamental, data_coverage, and provenance_summary with non-None values.
      // Serialize to context, deserialize, assert new fields are preserved.
  }
  ```

- [ ] 2.5 Add a round-trip test to `src/workflow/snapshot.rs`:

  ```rust
  #[tokio::test]
  async fn snapshot_preserves_evidence_and_reporting_fields() {
      // Save a TradingState with non-None evidence and coverage fields to a temp snapshot.
      // Load it back and assert the fields survive the round-trip.
  }
  ```

- [ ] 2.6 Run `cargo test --lib workflow::context_bridge -- --nocapture` → PASS.
  Run `cargo test --lib workflow::snapshot -- --nocapture` → PASS.
- [ ] 2.7 Commit: `git add src/state/trading_state.rs src/workflow/context_bridge.rs src/workflow/snapshot.rs && git commit -m "feat: extend TradingState with evidence and reporting fields"`

## 3. AnalystSyncTask Dual-Write

- [ ] 3.1 Add source-mapping helpers or module-level constants in `src/workflow/tasks/analyst.rs`:

  ```rust
  const EVIDENCE_SOURCES_FUNDAMENTAL: &[(&str, &str)] = &[
      ("finnhub", "fundamentals"),
  ];
  const EVIDENCE_SOURCES_SENTIMENT: &[(&str, &str)] = &[
      ("finnhub", "company_news_sentiment_inputs"),
  ];
  const EVIDENCE_SOURCES_NEWS: &[(&str, &str)] = &[
      ("finnhub", "company_news"),
      ("fred", "macro_indicators"),
  ];
  const EVIDENCE_SOURCES_TECHNICAL: &[(&str, &str)] = &[
      ("yfinance", "ohlcv"),
  ];
  ```

  Add a helper that converts `&[(&str, &str)]` pairs and a `fetched_at: &str` into `Vec<EvidenceSource>`,
  with `effective_at`, `symbol`, `url`, `citation`, `freshness_hours` all `None`.

- [ ] 3.2 In `AnalystSyncTask::run` (or equivalent execution method), after populating each legacy field,
  also populate the corresponding `evidence_*` field:

  ```rust
  // After: state.fundamental_metrics = Some(fundamental_data.clone());
  let fetched_at = chrono::Utc::now().to_rfc3339();
  state.evidence_fundamental = Some(EvidenceRecord {
      kind: EvidenceKind::Fundamental,
      payload: fundamental_data.clone(),
      sources: make_sources(EVIDENCE_SOURCES_FUNDAMENTAL, &fetched_at),
      quality_flags: vec![],
  });
  ```

  Repeat for `evidence_technical`, `evidence_sentiment`, `evidence_news`.
  Use `EvidenceKind::Technical`, `EvidenceKind::Sentiment`, `EvidenceKind::News` respectively.

- [ ] 3.3 Compute and write `DataCoverageReport`:

  ```rust
  let required = vec![
      "fundamentals".to_owned(),
      "sentiment".to_owned(),
      "news".to_owned(),
      "technical".to_owned(),
  ];
  let mut missing = vec![];
  if state.evidence_fundamental.is_none() { missing.push("fundamentals".to_owned()); }
  if state.evidence_sentiment.is_none()   { missing.push("sentiment".to_owned()); }
  if state.evidence_news.is_none()        { missing.push("news".to_owned()); }
  if state.evidence_technical.is_none()   { missing.push("technical".to_owned()); }
  state.data_coverage = Some(DataCoverageReport {
      required_inputs: required,
      missing_inputs: missing,
      stale_inputs: vec![],
      partial_inputs: vec![],
  });
  ```

- [ ] 3.4 Compute and write `ProvenanceSummary`:

  ```rust
  let mut providers: Vec<String> = vec![
      "finnhub".to_owned(),
      "fred".to_owned(),
      "yfinance".to_owned(),
  ];
  providers.sort();
  providers.dedup();
  state.provenance_summary = Some(ProvenanceSummary {
      providers_used: providers,
      generated_at: chrono::Utc::now().to_rfc3339(),
      caveats: vec![],
  });
  ```

  Note: always include all three providers in `providers_used` regardless of which analyst outputs are present,
  because the coverage report already tracks absence via `missing_inputs`.

- [ ] 3.5 Update the existing test `analyst_sync_all_succeed_returns_continue` in
  `src/workflow/tasks/tests.rs` to additionally assert:
  - `state.evidence_fundamental.is_some()`
  - `state.evidence_technical.is_some()`
  - `state.evidence_sentiment.is_some()`
  - `state.evidence_news.is_some()`
  - `state.data_coverage.as_ref().unwrap().missing_inputs.is_empty()`
  - `state.provenance_summary.as_ref().unwrap().providers_used == ["finnhub", "fred", "yfinance"]`

- [ ] 3.6 Add a new test `analyst_sync_marks_missing_inputs_in_coverage_report`:

  Construct a `TradingState` where only `fundamental_metrics` is populated (others absent). Run
  `AnalystSyncTask`. Assert `data_coverage.missing_inputs` contains `"sentiment"`, `"news"`, and `"technical"`
  in that order, and does not contain `"fundamentals"`.

- [ ] 3.7 Run `cargo test --lib workflow::tasks -- --nocapture` → PASS.
- [ ] 3.8 Commit: `git add src/workflow/tasks/analyst.rs src/workflow/tasks/tests.rs && git commit -m "feat: dual-write analyst evidence and coverage metadata in AnalystSyncTask"`

## 4. Prompt Context Builders and Downstream Injection

- [ ] 4.1 Add `build_evidence_context(state: &TradingState) -> String` to
  `src/agents/shared/prompt.rs`. Required output format:

  ```text
  Typed evidence snapshot:
  - fundamentals: <json or null>
  - sentiment: <json or null>
  - news: <json or null>
  - technical: <json or null>
  ```

  Use `serde_json::to_string(&state.evidence_fundamental).unwrap_or_else(|_| "null".to_owned())` for each
  field. Never panic; return a fallback string if the entire state is unavailable.

- [ ] 4.2 Add `build_data_quality_context(state: &TradingState) -> String` to
  `src/agents/shared/prompt.rs`. Required output format:

  ```text
  Data quality snapshot:
  - required_inputs: [...]
  - missing_inputs: [...]
  - providers_used: [...]
  ```

  Read from `state.data_coverage` and `state.provenance_summary`. Return a compact fallback string when both
  are `None`. Never panic.

- [ ] 4.3 Add unit tests in `src/agents/shared/prompt.rs` under `#[cfg(test)]`:
  - `evidence_context_with_populated_state_contains_fundamentals_key`: assert output contains `"fundamentals:"`.
  - `evidence_context_with_empty_state_returns_fallback`: pass a default `TradingState`, assert output is
    non-empty and does not panic.
  - `data_quality_context_with_populated_state_contains_required_inputs`: assert output contains
    `"required_inputs:"`.
  - `data_quality_context_with_empty_state_returns_fallback`: pass a default `TradingState`, assert
    non-empty, no panic.

- [ ] 4.4 In `src/agents/researcher/common.rs`, after the existing analyst-context block, append:

  ```rust
  let evidence_ctx = crate::agents::shared::prompt::build_evidence_context(state);
  let quality_ctx = crate::agents::shared::prompt::build_data_quality_context(state);
  // Append both to the system prompt string.
  // Also append:
  // "Separate observed facts from interpretation."
  // "Surface unresolved uncertainty when evidence is weak or incomplete."
  ```

- [ ] 4.5 Apply the same append-only injection to `src/agents/risk/common.rs`.
- [ ] 4.6 Apply the same append-only injection to `src/agents/trader/mod.rs`, adding:

  ```text
  Missing or sparse upstream evidence must be acknowledged directly in your trade proposal.
  ```

- [ ] 4.7 Apply the same append-only injection to `src/agents/fund_manager/prompt.rs`, adding:

  ```text
  Data quality limits must be surfaced in the final rationale.
  ```

- [ ] 4.8 Add string-contains tests in each of the four modified agent files asserting:
  - The output of `build_evidence_context(...)` is included in the rendered prompt.
  - The output of `build_data_quality_context(...)` is included in the rendered prompt.
  - The prompt contains `"Separate observed facts from interpretation."`.
  - The prompt contains `"Surface unresolved uncertainty when evidence is weak or incomplete."`.

- [ ] 4.9 Run:
  - `cargo test --lib agents::shared::prompt -- --nocapture` → PASS
  - `cargo test --lib agents::researcher -- --nocapture` → PASS
  - `cargo test --lib agents::risk -- --nocapture` → PASS
  - `cargo test --lib agents::trader -- --nocapture` → PASS
  - `cargo test --lib agents::fund_manager -- --nocapture` → PASS

- [ ] 4.10 Commit: `git add src/agents/shared/prompt.rs src/agents/researcher src/agents/risk src/agents/trader src/agents/fund_manager && git commit -m "feat: inject typed evidence and quality context into downstream prompts"`

## 5. Verification

- [ ] 5.1 Run `cargo fmt -- --check` — fix any formatting issues.
- [ ] 5.2 Run `cargo clippy --all-targets -- -D warnings` — resolve all warnings.
- [ ] 5.3 Run `cargo nextest run --all-features --locked` — all tests pass.
- [ ] 5.4 Run `cargo build` — clean compilation with no unused-import or dead-code warnings.
- [ ] 5.5 After all tasks complete, run `/opsx:verify`.
