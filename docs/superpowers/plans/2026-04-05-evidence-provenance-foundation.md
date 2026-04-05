# Evidence and Provenance Foundation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the first delivery slice of the financial-services-plugins-inspired architecture: README attribution, prompt/source-discipline hardening, preflight entity resolution and config-derived capabilities, evidence/provenance/coverage state, and final report coverage/provenance sections.

**Architecture:** Add one new `PreflightTask` before analyst fan-out, keep the existing five-phase graph intact, and dual-write new typed evidence/provenance fields alongside the current analyst output structs. Use the existing symbol validator in `src/data/symbol.rs`, the existing context bridge in `src/workflow/context_bridge.rs`, and the existing snapshot store in `src/workflow/snapshot.rs` as the integration boundaries.

**Tech Stack:** Rust 1.93+, `serde`, `schemars`, `tokio`, `graph-flow`, `rig-core`, existing SQLite snapshots via `sqlx`, existing report rendering code.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `README.md` | Modify | Mention `financial-services-plugins` as an additional inspiration |
| `docs/prompts.md` | Modify | Add source hierarchy and missing-data prompt rules |
| `config.toml` | Modify | Add default `[enrichment]` section |
| `src/agents/shared/prompt.rs` | Modify | Shared rule helpers plus `build_evidence_context` and `build_data_quality_context` |
| `src/agents/analyst/fundamental.rs` | Modify | Apply stronger analyst prompt rules |
| `src/agents/analyst/news.rs` | Modify | Apply stronger analyst prompt rules |
| `src/agents/analyst/sentiment.rs` | Modify | Apply stronger analyst prompt rules |
| `src/agents/analyst/technical.rs` | Modify | Apply stronger analyst prompt rules |
| `src/agents/researcher/common.rs` | Modify | Inject evidence and data-quality context |
| `src/agents/risk/common.rs` | Modify | Inject evidence and data-quality context |
| `src/agents/trader/mod.rs` | Modify | Inject evidence and data-quality context |
| `src/agents/fund_manager/prompt.rs` | Modify | Inject evidence and data-quality context |
| `src/config.rs` | Modify | Add `DataEnrichmentConfig` with defaults |
| `src/data/entity.rs` | Create | Canonical instrument resolution on top of `src/data/symbol.rs` |
| `src/data/adapters/mod.rs` | Create | `ProviderCapabilities` and adapter exports |
| `src/data/adapters/transcripts.rs` | Create | `TranscriptEvidence` and `TranscriptProvider` |
| `src/data/adapters/estimates.rs` | Create | `ConsensusEvidence` and `EstimatesProvider` |
| `src/data/adapters/events.rs` | Create | `EventNewsEvidence` and `EventNewsProvider` |
| `src/data/mod.rs` | Modify | Re-export entity and adapters modules |
| `src/state/evidence.rs` | Create | `EvidenceKind` and `EvidenceRecord<T>` |
| `src/state/provenance.rs` | Create | `EvidenceSource` and `DataQualityFlag` |
| `src/state/reporting.rs` | Create | `DataCoverageReport` and `ProvenanceSummary` |
| `src/state/mod.rs` | Modify | Export new state modules |
| `src/state/trading_state.rs` | Modify | Add evidence and reporting fields |
| `src/workflow/tasks/common.rs` | Modify | Add preflight context keys |
| `src/workflow/tasks/preflight.rs` | Create | Validate symbol, write resolved instrument, capabilities, coverage, cache placeholders |
| `src/workflow/tasks/mod.rs` | Modify | Export `PreflightTask`, context keys, and `preflight_tests` |
| `src/workflow/pipeline.rs` | Modify | Insert `PreflightTask` before analyst fan-out |
| `src/workflow/context_bridge.rs` | Modify | Add explicit round-trip test for expanded `TradingState` |
| `src/workflow/snapshot.rs` | Modify | Persist expanded `TradingState` |
| `src/workflow/tasks/preflight_tests.rs` | Create | Focused tests for preflight and pipeline ordering |
| `src/workflow/tasks/tests.rs` | Modify | Focused tests for `AnalystSyncTask` dual-write and coverage |
| `src/workflow/tasks/analyst.rs` | Modify | Read canonical symbol from context and dual-write evidence/provenance/coverage |
| `src/workflow/tasks/test_helpers.rs` | Modify | Add a preflight stub if the test seam requires it |
| `src/report/coverage.rs` | Create | Render `Data Quality and Coverage` section |
| `src/report/provenance.rs` | Create | Render `Evidence Provenance` section |
| `src/report/final_report.rs` | Modify | Call the new report helpers |
| `src/report/mod.rs` | Modify | Export the report module shape cleanly |

---

## Chunk 1: Docs and Shared Prompt Rules

### Task 1: Update README and prompt docs

**Files:**
- Modify: `README.md`
- Modify: `docs/prompts.md`

- [ ] **Step 1: Update the README intro paragraph**

Edit the first project-description paragraph so it mentions both TradingAgents and Anthropic's
`financial-services-plugins` as inspirations.

Use wording close to:

```md
Scorpio-Analyst is a Rust-native reimplementation of the TradingAgents framework, inspired by the paper
_TradingAgents: Multi-Agents LLM Financial Trading Framework_. It is also informed by reusable financial analysis and
reporting patterns from Anthropic's financial-services-plugins repository, especially around evidence handling,
provenance, and modular financial workflows.
```

- [ ] **Step 2: Add global prompt rules to `docs/prompts.md`**

Under `## Global Prompt Rules`, add rules for:

```md
- Prefer authoritative runtime evidence over inference or memory.
- If required data is missing, return schema-compatible `null`, `[]`, or explicit sparse summaries instead of guessing.
- Distinguish observed facts from interpretation.
- Missing or sparse evidence must lower confidence explicitly.
- Let Rust compute deterministic comparisons and ranges; use the model to interpret them.
```

- [ ] **Step 3: Verify the doc changes are present**

Run: `rg -n "financial-services-plugins|authoritative runtime evidence|Let Rust compute" README.md docs/prompts.md`

Expected: matches in both files.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/prompts.md
git commit -m "docs: credit financial-services-plugins inspiration and harden prompt rules"
```

### Task 2: Add shared prompt helpers in `src/agents/shared/prompt.rs`

**Files:**
- Modify: `src/agents/shared/prompt.rs`

- [ ] **Step 1: Add the three static rule helpers**

Add helpers shaped like:

```rust
pub(crate) fn build_authoritative_source_prompt_rule() -> &'static str {
    "Prefer authoritative runtime evidence over inference. If the runtime does not provide a value, do not invent it."
}

pub(crate) fn build_missing_data_prompt_rule() -> &'static str {
    "If data is missing, return schema-compatible nulls or empty collections and explicitly acknowledge the gap."
}

pub(crate) fn build_data_quality_prompt_rule() -> &'static str {
    "Treat missing or sparse evidence as part of the decision context, not optional commentary."
}
```

- [ ] **Step 2: Add `build_evidence_context` and `build_data_quality_context` with explicit output contracts**

Add:

```rust
pub(crate) fn build_evidence_context(state: &TradingState) -> String;
pub(crate) fn build_data_quality_context(state: &TradingState) -> String;
```

Use this exact first-slice rendering contract:

- `build_evidence_context(state)` returns:

```text
Typed evidence snapshot:
- fundamentals: <json or null>
- sentiment: <json or null>
- news: <json or null>
- technical: <json or null>
```

- `build_data_quality_context(state)` returns:

```text
Data quality snapshot:
- required_inputs: [...]
- missing_inputs: [...]
- providers_used: [...]
```

If the corresponding state is absent, each helper must return a compact fallback string instead of panicking.

- [ ] **Step 3: Add unit tests in `src/agents/shared/prompt.rs`**

Add tests named:

```rust
#[test]
fn authority_rule_mentions_runtime_evidence() { ... }

#[test]
fn missing_data_rule_mentions_null_or_empty() { ... }

#[test]
fn quality_rule_mentions_missing_or_sparse_evidence() { ... }

#[test]
fn evidence_context_handles_empty_state() { ... }

#[test]
fn data_quality_context_handles_empty_state() { ... }
```

- [ ] **Step 4: Run the prompt helper tests**

Run: `cargo test --lib agents::shared::prompt -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agents/shared/prompt.rs
git commit -m "feat: add shared prompt helpers for evidence and data quality"
```

### Task 3: Apply stronger prompt rules to analyst prompts

**Files:**
- Modify: `src/agents/analyst/fundamental.rs`
- Modify: `src/agents/analyst/news.rs`
- Modify: `src/agents/analyst/sentiment.rs`
- Modify: `src/agents/analyst/technical.rs`

- [ ] **Step 1: Append the shared rule strings to analyst prompts**

Import the shared rule helpers and append them to each analyst system prompt.

- [ ] **Step 2: Add explicit unsupported-inference rules**

For each analyst prompt, add lines equivalent to:

```text
Do not infer estimates, transcript commentary, or quarter labels unless the runtime provides them.
If evidence is sparse or missing, say so explicitly in `summary` rather than padding weak claims.
Separate observed facts from interpretation.
```

- [ ] **Step 3: Add or update prompt-rendering tests**

Add focused string-contains tests in each modified analyst file asserting the new rule text is present, including:

- authoritative runtime evidence
- do not infer unsupported data
- separate observed facts from interpretation

- [ ] **Step 4: Run analyst tests**

Run: `cargo test --lib agents::analyst -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agents/analyst
git commit -m "feat: harden analyst prompts around missing and unsupported evidence"
```

---

## Chunk 2: Config, Entity Resolution, and Preflight

### Task 4: Add `DataEnrichmentConfig` to `src/config.rs` and `config.toml`

**Files:**
- Modify: `src/config.rs`
- Modify: `config.toml`

- [ ] **Step 1: Add `DataEnrichmentConfig` to `src/config.rs`**

Add:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct DataEnrichmentConfig {
    #[serde(default)]
    pub enable_transcripts: bool,
    #[serde(default)]
    pub enable_consensus_estimates: bool,
    #[serde(default)]
    pub enable_event_news: bool,
    #[serde(default = "default_max_evidence_age_hours")]
    pub max_evidence_age_hours: u64,
}

fn default_max_evidence_age_hours() -> u64 {
    48
}
```

Attach it to `Config` as:

```rust
#[serde(default)]
pub enrichment: DataEnrichmentConfig,
```

- [ ] **Step 2: Add config tests in `src/config.rs`**

Add tests named:

```rust
#[test]
fn enrichment_config_defaults_are_safe() { ... }

#[test]
fn load_from_accepts_enrichment_section() { ... }

#[test]
fn env_overrides_apply_to_enrichment_fields() { ... }
```

- [ ] **Step 3: Add default config to `config.toml`**

Append:

```toml
[enrichment]
enable_transcripts = false
enable_consensus_estimates = false
enable_event_news = false
max_evidence_age_hours = 48
```

- [ ] **Step 4: Run config tests**

Run: `cargo test --lib config -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs config.toml
git commit -m "feat: add data enrichment configuration"
```

### Task 5: Create `src/data/entity.rs` using the existing symbol validator

**Files:**
- Create: `src/data/entity.rs`
- Modify: `src/data/mod.rs`

- [ ] **Step 1: Implement `ResolvedInstrument` and `resolve_symbol`**

Create `src/data/entity.rs` and make `resolve_symbol` call the existing validator in `src/data/symbol.rs`.

Use a shape like:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedInstrument {
    pub input_symbol: String,
    pub canonical_symbol: String,
    pub issuer_name: Option<String>,
    pub exchange: Option<String>,
    pub instrument_type: Option<String>,
    pub aliases: Vec<String>,
}

pub fn resolve_symbol(symbol: &str) -> Result<ResolvedInstrument, TradingError> {
    let validated = super::symbol::validate_symbol(symbol)?;
    Ok(ResolvedInstrument {
        input_symbol: symbol.to_owned(),
        canonical_symbol: validated.to_ascii_uppercase(),
        issuer_name: None,
        exchange: None,
        instrument_type: None,
        aliases: Vec::new(),
    })
}
```

- [ ] **Step 2: Export the new module from `src/data/mod.rs`**

Add:

```rust
pub mod entity;
pub use entity::{ResolvedInstrument, resolve_symbol};
```

- [ ] **Step 3: Add unit tests in `src/data/entity.rs`**

Add:

```rust
#[test]
fn resolve_symbol_trims_and_uppercases_input() { ... }

#[test]
fn resolve_symbol_preserves_original_input() { ... }

#[test]
fn resolve_symbol_rejects_empty_input() { ... }
```

- [ ] **Step 4: Run entity tests**

Run: `cargo test --lib data::entity -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/data/entity.rs src/data/mod.rs
git commit -m "feat: add canonical instrument resolution on top of symbol validation"
```

### Task 6: Create provider-agnostic adapter contracts

**Files:**
- Create: `src/data/adapters/mod.rs`
- Create: `src/data/adapters/transcripts.rs`
- Create: `src/data/adapters/estimates.rs`
- Create: `src/data/adapters/events.rs`
- Modify: `src/data/mod.rs`

- [ ] **Step 1: Define `ProviderCapabilities` in `src/data/adapters/mod.rs`**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub transcripts_enabled: bool,
    pub consensus_estimates_enabled: bool,
    pub event_news_enabled: bool,
}
```

- [ ] **Step 2: Define `TranscriptEvidence` and `TranscriptProvider`**

In `src/data/adapters/transcripts.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEvidence {
    pub period_label: Option<String>,
    pub published_at: Option<String>,
    pub speakers: Vec<String>,
    pub key_points: Vec<String>,
}

#[async_trait]
pub trait TranscriptProvider {
    async fn latest_transcript(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<TranscriptEvidence, TradingError>;
}
```

- [ ] **Step 3: Define `ConsensusEvidence` and `EstimatesProvider`**

In `src/data/adapters/estimates.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsensusEvidence {
    pub period_label: Option<String>,
    pub revenue_estimate: Option<f64>,
    pub eps_estimate: Option<f64>,
    pub analyst_count: Option<u32>,
    pub published_at: Option<String>,
}

#[async_trait]
pub trait EstimatesProvider {
    async fn latest_consensus(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<ConsensusEvidence, TradingError>;
}
```

- [ ] **Step 4: Define `EventNewsEvidence` and `EventNewsProvider`**

In `src/data/adapters/events.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventNewsEvidence {
    pub event_type: Option<String>,
    pub headline: String,
    pub published_at: Option<String>,
    pub summary: Option<String>,
    pub relevance_score: Option<f64>,
}

#[async_trait]
pub trait EventNewsProvider {
    async fn event_feed(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<Vec<EventNewsEvidence>, TradingError>;
}
```

- [ ] **Step 5: Export the adapters through `src/data/adapters/mod.rs` and `src/data/mod.rs`**

Add public module exports and re-exports so workflow code can import these contracts from `crate::data`.

- [ ] **Step 6: Add serde tests in each new adapter file**

Add one small serialize/deserialize round-trip test for each evidence struct.

- [ ] **Step 7: Run adapter tests**

Run: `cargo test --lib data::adapters -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/data/adapters src/data/mod.rs
git commit -m "feat: add provider-agnostic enrichment contracts"
```

### Task 7: Add `PreflightTask` and make analysts consume canonical symbols

**Files:**
- Create: `src/workflow/tasks/preflight.rs`
- Modify: `src/workflow/tasks/common.rs`
- Modify: `src/workflow/tasks/mod.rs`
- Modify: `src/workflow/pipeline.rs`
- Modify: `src/workflow/tasks/analyst.rs`
- Create: `src/workflow/tasks/preflight_tests.rs`
- Modify: `src/workflow/tasks/test_helpers.rs`

- [ ] **Step 1: Add the preflight context keys**

In `src/workflow/tasks/common.rs`, add:

```rust
pub const KEY_RESOLVED_INSTRUMENT: &str = "resolved_instrument";
pub const KEY_PROVIDER_CAPABILITIES: &str = "provider_capabilities";
pub const KEY_REQUIRED_COVERAGE_INPUTS: &str = "required_coverage_inputs";
pub const KEY_CACHED_TRANSCRIPT: &str = "cached_transcript";
pub const KEY_CACHED_CONSENSUS: &str = "cached_consensus";
pub const KEY_CACHED_EVENT_FEED: &str = "cached_event_feed";
```

- [ ] **Step 2: Implement `PreflightTask`**

Create `src/workflow/tasks/preflight.rs` with a task that:

1. deserializes `TradingState`
2. resolves the symbol with `resolve_symbol`
3. writes serialized `ResolvedInstrument` to `KEY_RESOLVED_INSTRUMENT`
4. builds `ProviderCapabilities` from `config.enrichment`
5. writes serialized `ProviderCapabilities` to `KEY_PROVIDER_CAPABILITIES`
6. writes serialized `Vec<String>` baseline coverage ids to `KEY_REQUIRED_COVERAGE_INPUTS`
7. writes serialized `Option::<TranscriptEvidence>::None` to `KEY_CACHED_TRANSCRIPT`
8. writes serialized `Option::<ConsensusEvidence>::None` to `KEY_CACHED_CONSENSUS`
9. writes serialized `Option::<Vec<EventNewsEvidence>>::None` to `KEY_CACHED_EVENT_FEED`
10. returns `NextAction::Continue`

If `resolve_symbol` returns an error, `PreflightTask` must fail hard.

- [ ] **Step 3: Export `PreflightTask` and wire the test module**

In `src/workflow/tasks/mod.rs`:

1. add `mod preflight;`
2. add `#[cfg(test)] mod preflight_tests;`
3. re-export `PreflightTask`
4. re-export the new key constants from `common`

- [ ] **Step 4: Insert `PreflightTask` into `src/workflow/pipeline.rs`**

Add a task id constant, include it in `REPLACEABLE_TASK_IDS`, instantiate it in `build_graph_impl`, and add the edge:

`preflight -> analyst_fanout`

Update the pipeline topology comment to show `PreflightTask` as the first node.

Also update the current start-task call sites so execution really begins at preflight:

- `graph.set_start_task(...)`
- `Session::new_from_task(...)`

- [ ] **Step 5: Make analyst tasks consume the canonical symbol**

In `src/workflow/tasks/analyst.rs`, add a helper that reads `KEY_RESOLVED_INSTRUMENT` from context and deserializes
`ResolvedInstrument`.

Update each analyst task to use `resolved.canonical_symbol.clone()` instead of `state.asset_symbol.clone()` when
constructing the analyst.

If the key is missing after `PreflightTask`, treat it as orchestration corruption and return an error.

- [ ] **Step 6: Add focused tests in `src/workflow/tasks/preflight_tests.rs`**

Add tests named:

```rust
#[tokio::test]
async fn preflight_writes_resolved_instrument() { ... }

#[tokio::test]
async fn preflight_writes_provider_capabilities() { ... }

#[tokio::test]
async fn preflight_writes_required_coverage_inputs() { ... }

#[tokio::test]
async fn preflight_seeds_cached_enrichment_keys_with_null() { ... }

#[tokio::test]
async fn preflight_rejects_invalid_symbol() { ... }

#[tokio::test]
async fn pipeline_orders_preflight_before_analyst_fanout() { ... }
```

If the task test seam stubs every workflow task, add a simple preflight stub in `src/workflow/tasks/test_helpers.rs`.

- [ ] **Step 7: Run workflow tests**

Run: `cargo test --lib workflow::tasks -- --nocapture`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/workflow/tasks src/workflow/pipeline.rs
git commit -m "feat: add preflight task and canonical symbol flow"
```

---

## Chunk 3: Evidence, Provenance, Sync, and Prompt Consumption

### Task 8: Add new state modules for evidence, provenance, and reporting

**Files:**
- Create: `src/state/evidence.rs`
- Create: `src/state/provenance.rs`
- Create: `src/state/reporting.rs`
- Modify: `src/state/mod.rs`

- [ ] **Step 1: Create `src/state/provenance.rs`**

Add exactly:

```rust
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

- [ ] **Step 2: Create `src/state/evidence.rs`**

Add exactly:

```rust
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
pub struct EvidenceRecord<T> {
    pub kind: EvidenceKind,
    pub payload: T,
    pub sources: Vec<EvidenceSource>,
    pub quality_flags: Vec<DataQualityFlag>,
}
```

- [ ] **Step 3: Create `src/state/reporting.rs`**

Add exactly:

```rust
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

- [ ] **Step 4: Export the new modules from `src/state/mod.rs`**

Add the `mod` declarations and `pub use` lines.

- [ ] **Step 5: Add serde round-trip tests in each new file**

Each new file should have at least one round-trip test proving its main types serialize and deserialize cleanly.

- [ ] **Step 6: Run state tests**

Run: `cargo test --lib state -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/state/evidence.rs src/state/provenance.rs src/state/reporting.rs src/state/mod.rs
git commit -m "feat: add evidence provenance and reporting state modules"
```

### Task 9: Extend `TradingState`, context bridge, and snapshots

**Files:**
- Modify: `src/state/trading_state.rs`
- Modify: `src/workflow/context_bridge.rs`
- Modify: `src/workflow/snapshot.rs`

- [ ] **Step 1: Extend `TradingState` with the new fields**

Add:

```rust
pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
pub evidence_news: Option<EvidenceRecord<NewsData>>,
pub data_coverage: Option<DataCoverageReport>,
pub provenance_summary: Option<ProvenanceSummary>,
```

Initialize each to `None` in `TradingState::new`.

Also update any exhaustive `TradingState { ... }` literals that compilation surfaces. The most likely spillover files are:

- `src/workflow/context_bridge.rs`
- `src/workflow/snapshot.rs`
- `src/workflow/tasks/tests.rs`
- `src/agents/researcher/common.rs`
- `src/agents/risk/common.rs`

Do not stop at the listed files if `cargo build` reveals additional literal initializers.

- [ ] **Step 2: Add an explicit round-trip test in `src/workflow/context_bridge.rs`**

Extend the test module in `src/workflow/context_bridge.rs` with a case that populates
`evidence_fundamental`, `data_coverage`, and `provenance_summary`, serializes the state to context, deserializes it, and
asserts the new fields round-trip correctly.

- [ ] **Step 3: Add a snapshot round-trip test in `src/workflow/snapshot.rs`**

Extend the snapshot tests with a case that saves a `TradingState` populated with the new fields and loads it back.

- [ ] **Step 4: Run bridge and snapshot tests**

Run: `cargo test --lib workflow::context_bridge -- --nocapture`

Run: `cargo test --lib workflow::snapshot -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/state/trading_state.rs src/workflow/context_bridge.rs src/workflow/snapshot.rs
git commit -m "feat: persist evidence and reporting fields in TradingState"
```

### Task 10: Dual-write evidence and coverage in `AnalystSyncTask`

**Files:**
- Modify: `src/workflow/tasks/analyst.rs`
- Modify: `src/workflow/tasks/tests.rs`

- [ ] **Step 1: Add exact source-mapping helpers in `src/workflow/tasks/analyst.rs`**

Add small helpers or constants for the exact first-slice mapping:

- fundamentals -> `EvidenceKind::Fundamental`, source `("finnhub", "fundamentals")`
- sentiment -> `EvidenceKind::Sentiment`, source `("finnhub", "company_news_sentiment_inputs")`
- news -> sources `("finnhub", "company_news")` and `("fred", "macro_indicators")`
- technical -> `EvidenceKind::Technical`, source `("yfinance", "ohlcv")`

Use RFC3339 UTC for `fetched_at` and an empty `quality_flags` vector in the first slice.

- [ ] **Step 2: Update `AnalystSyncTask` to dual-write legacy and new fields**

When an analyst output is present, continue populating the legacy field and also populate the corresponding `evidence_*`
field.

- [ ] **Step 3: Compute `DataCoverageReport` and `ProvenanceSummary`**

Use this exact first-slice logic:

- `required_inputs`: `fundamentals`, `sentiment`, `news`, `technical`
- `missing_inputs`: whichever `evidence_*` fields are `None`, using this exact mapping:
  - `evidence_fundamental` -> `fundamentals`
  - `evidence_sentiment` -> `sentiment`
  - `evidence_news` -> `news`
  - `evidence_technical` -> `technical`
- `stale_inputs`: `[]`
- `partial_inputs`: `[]`
- `providers_used`: dedupe provider names from the configured evidence-source mappings above
- `generated_at`: current UTC RFC3339 timestamp
- `caveats`: `[]` in the first slice

- [ ] **Step 4: Extend `src/workflow/tasks/tests.rs`**

Update `analyst_sync_all_succeed_returns_continue` to assert the new `evidence_*`, `data_coverage`, and
`provenance_summary` fields are populated.

Add:

```rust
#[tokio::test]
async fn analyst_sync_marks_missing_inputs_in_coverage_report() { ... }
```

- [ ] **Step 5: Run analyst-sync tests**

Run: `cargo test --lib workflow::tasks::tests::analyst_sync_all_succeed_returns_continue -- --nocapture`

Run: `cargo test --lib workflow::tasks -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/workflow/tasks/analyst.rs src/workflow/tasks/tests.rs
git commit -m "feat: dual-write analyst evidence and coverage metadata"
```

### Task 11: Wire evidence and quality context into downstream prompts

**Files:**
- Modify: `src/agents/researcher/common.rs`
- Modify: `src/agents/risk/common.rs`
- Modify: `src/agents/trader/mod.rs`
- Modify: `src/agents/fund_manager/prompt.rs`

- [ ] **Step 1: Append `build_evidence_context(state)` and `build_data_quality_context(state)` to researcher prompts**

In `src/agents/researcher/common.rs`, inject both shared context builders after the current analyst-context block.

- [ ] **Step 2: Append the same shared context to risk prompts**

In `src/agents/risk/common.rs`, inject both builders after the analyst snapshot block.

- [ ] **Step 3: Update trader and fund-manager prompts explicitly**

In `src/agents/trader/mod.rs`, append the evidence/data-quality context and add a short instruction that missing or
sparse upstream evidence must be acknowledged directly.

In `src/agents/fund_manager/prompt.rs`, append the evidence/data-quality context and add a short instruction that data
quality limits must be surfaced in the final rationale.

- [ ] **Step 4: Ensure the prompt text covers facts vs interpretation and unresolved uncertainty**

Across the four modified modules, make sure the prompt text explicitly says:

```text
Separate observed facts from interpretation.
Surface unresolved uncertainty when evidence is weak or incomplete.
```

- [ ] **Step 5: Add or update tests for prompt rendering**

In each of these files:

- `src/agents/researcher/common.rs`
- `src/agents/risk/common.rs`
- `src/agents/trader/mod.rs`
- `src/agents/fund_manager/prompt.rs`

add focused string-contains tests asserting that:

- `build_evidence_context(...)` output is included
- `build_data_quality_context(...)` output is included
- the prompt text contains `Separate observed facts from interpretation.`
- the prompt text contains `Surface unresolved uncertainty when evidence is weak or incomplete.`

- [ ] **Step 6: Run downstream prompt tests**

Run: `cargo test --lib agents::researcher -- --nocapture`

Run: `cargo test --lib agents::risk -- --nocapture`

Run: `cargo test --lib agents::trader -- --nocapture`

Run: `cargo test --lib agents::fund_manager -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/agents/researcher src/agents/risk src/agents/trader src/agents/fund_manager
git commit -m "feat: inject typed evidence and quality context into downstream prompts"
```

---

## Chunk 4: Report Sections and Final Verification

### Task 12: Add report coverage and provenance sections

**Files:**
- Create: `src/report/coverage.rs`
- Create: `src/report/provenance.rs`
- Modify: `src/report/final_report.rs`
- Modify: `src/report/mod.rs`

- [ ] **Step 1: Create `src/report/coverage.rs`**

Add:

```rust
pub(crate) fn write_data_quality_and_coverage(out: &mut String, state: &TradingState) { ... }
```

Behavior:

- heading must be exactly `Data Quality and Coverage`
- if `state.data_coverage` is `None`, write `Unavailable`
- otherwise list required inputs and missing inputs at minimum

- [ ] **Step 2: Create `src/report/provenance.rs`**

Add:

```rust
pub(crate) fn write_evidence_provenance(out: &mut String, state: &TradingState) { ... }
```

Behavior:

- heading must be exactly `Evidence Provenance`
- if `state.provenance_summary` is `None`, write `Unavailable`
- otherwise list providers used and caveats at minimum

- [ ] **Step 3: Call the new helpers from `format_final_report`**

Insert both sections after the analyst snapshot and before the debate/risk sections.

- [ ] **Step 4: Add report tests**

Add tests that assert:

1. the two new headings are present when data exists
2. the exact fallback string `Unavailable` appears when the backing fields are `None`

- [ ] **Step 5: Run report tests**

Run: `cargo test --lib report -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/report
git commit -m "feat: add report coverage and provenance sections"
```

### Task 13: Run full verification

**Files:**
- No code changes expected

- [ ] **Step 1: Run formatting**

Run: `cargo fmt -- --check`

Expected: no diffs.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`

Expected: PASS.

- [ ] **Step 3: Run full tests**

Run: `cargo nextest run --all-features --locked`

Expected: PASS.

- [ ] **Step 4: Manual smoke test**

Run: `cargo run`

Prerequisite: valid local API keys in `.env` or environment variables for the configured providers.

Expected: the pipeline still completes and the final report contains `Data Quality and Coverage` and `Evidence Provenance`.

---

## Follow-On Plans

Do not extend this plan further. Write separate follow-on implementation plans for:

1. thesis memory
2. peer/comps and scenario valuation
3. concrete earnings/event enrichment providers
4. analysis pack extraction

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-05-evidence-provenance-foundation.md`. Ready to execute?
