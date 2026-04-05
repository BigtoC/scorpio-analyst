# Financial-Services-Plugins-Inspired Architecture Evolution

## Context

`scorpio-analyst` is a Rust-native multi-agent trading system built around:

- typed shared state in `src/state/`
- `graph-flow` orchestration in `src/workflow/`
- `rig-core` agents in `src/agents/`
- Finnhub, FRED, and Yahoo Finance data adapters in `src/data/`

The current business workflow remains correct and is not replaced by this design:

`Analyst -> Research Debate -> Trader -> Risk Debate -> Fund Manager`

This design is influenced by two upstream projects:

- [TradingAgents](https://github.com/TauricResearch/TradingAgents): the multi-agent trading-firm workflow
- [Anthropic financial-services-plugins](https://github.com/anthropics/financial-services-plugins): reusable financial workflows, evidence discipline, provenance reporting, and event-driven analysis patterns

`README.md` should explicitly mention both inspirations.

## Goals

1. Improve analyst quality with explicit evidence, provenance, and data-quality handling.
2. Add a provider-agnostic enrichment seam for transcripts, consensus estimates, and event/news feeds.
3. Move deterministic finance transformations into Rust instead of leaving them implicit inside prompts.
4. Improve final reports with coverage and provenance sections.
5. Preserve the current five-phase workflow while making the runtime more modular.
6. Provide a clear path for later additions: thesis memory, scenario valuation, concrete enrichment providers, and analysis packs.

## Non-Goals

1. Replace `graph-flow` or the existing five-phase topology.
2. Introduce vendor-specific enrichment providers in the architecture layer.
3. Rebuild Scorpio around markdown skills or MCP manifests.
4. Remove legacy analyst state fields in the first rollout.
5. Add web, TUI, Office, or browser-based reporting as part of this change.

## Recommended Order

The recommended implementation order is:

1. Prompt and source-discipline contracts
2. Entity resolution and provider capability layer
3. Evidence, provenance, and coverage state model
4. Report provenance and data-quality sections
5. Thesis memory
6. Peer/comps and scenario valuation
7. Earnings/event enrichment
8. Analysis pack extraction

This is a hybrid roadmap:

- Stage 1 strengthens the current runtime in place.
- Stage 2 extracts stable concepts into pack-driven configuration.

## First Implementation Boundary

The first implementation slice covers only the following work:

1. documentation alignment
2. prompt/source-discipline hardening
3. entity resolution and config-derived provider capabilities
4. evidence/provenance/coverage state
5. `PreflightTask`, `AnalystSyncTask`, and final-report updates for coverage/provenance

The first implementation slice explicitly does **not** include:

- thesis memory
- scenario valuation
- concrete transcript/estimate/event providers
- analysis packs

## Stage 1 Workflow

### Graph Shape

The current graph in `src/workflow/pipeline.rs` changes from:

`analyst_fanout -> analyst_sync -> ...`

to:

`preflight -> analyst_fanout -> analyst_sync -> ...`

Everything after `analyst_sync` remains unchanged in Stage 1.

### `PreflightTask`

New file:

- `src/workflow/tasks/preflight.rs`

`PreflightTask` responsibilities in the first implementation slice are exactly:

1. validate and canonicalize the input symbol
2. write the canonical instrument record to workflow context
3. write config-derived enrichment capability flags to workflow context
4. write baseline coverage expectations to workflow context
5. seed enrichment cache keys with explicit empty placeholders

`PreflightTask` does **not** load thesis memory in the first implementation slice.

### Shared Context Contracts

New keys live in `src/workflow/tasks/common.rs`.

Required Stage 1 keys:

- `KEY_RESOLVED_INSTRUMENT`
- `KEY_PROVIDER_CAPABILITIES`
- `KEY_REQUIRED_COVERAGE_INPUTS`
- `KEY_CACHED_TRANSCRIPT`
- `KEY_CACHED_CONSENSUS`
- `KEY_CACHED_EVENT_FEED`

`KEY_PREVIOUS_THESIS` is deferred to the thesis-memory follow-on milestone and must not be part of the first slice.

Serialization contract for Stage 1:

- `KEY_RESOLVED_INSTRUMENT` stores `serde_json` for `ResolvedInstrument`
- `KEY_PROVIDER_CAPABILITIES` stores `serde_json` for `ProviderCapabilities`
- `KEY_REQUIRED_COVERAGE_INPUTS` stores `serde_json` for `Vec<String>`
- `KEY_CACHED_TRANSCRIPT` stores `serde_json` for `Option<TranscriptEvidence>`
- `KEY_CACHED_CONSENSUS` stores `serde_json` for `Option<ConsensusEvidence>`
- `KEY_CACHED_EVENT_FEED` stores `serde_json` for `Option<Vec<EventNewsEvidence>>`

Stage 1 empty-placeholder semantics:

- each `KEY_CACHED_*` value is present in context
- the empty value is the literal JSON string `null`
- consumers must interpret `null` as "no fetched enrichment payload yet"
- missing `KEY_CACHED_*` after `PreflightTask` is orchestration corruption, not normal absence

### Stage 1 Coverage Policy

The only required inputs in the first implementation slice are the existing four analyst outputs:

- `fundamentals`
- `sentiment`
- `news`
- `technical`

These identifiers are fixed snake_case strings and are the only valid coverage ids in Stage 1.

Exact Stage 1 coverage-id mapping:

| Coverage ID    | Legacy Field           | New Evidence Field     | Report Label |
|----------------|------------------------|------------------------|--------------|
| `fundamentals` | `fundamental_metrics`  | `evidence_fundamental` | Fundamentals |
| `sentiment`    | `market_sentiment`     | `evidence_sentiment`   | Sentiment    |
| `news`         | `macro_news`           | `evidence_news`        | News         |
| `technical`    | `technical_indicators` | `evidence_technical`   | Technical    |

Stage 1 fail-open vs fail-closed policy:

- fail closed on invalid symbol input or orchestration corruption
- fail open on all enrichment-category absence or enrichment fetch failures
- no enrichment category is required in the first implementation slice

## Data Layer Contracts

### Config Contract

`DataEnrichmentConfig` lives in `src/config.rs` and must be attached to `Config` as:

```rust
#[serde(default)]
pub enrichment: DataEnrichmentConfig,
```

Required type:

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
```

Default config values in `config.toml`:

```toml
[enrichment]
enable_transcripts = false
enable_consensus_estimates = false
enable_event_news = false
max_evidence_age_hours = 48
```

Environment overrides follow the existing `SCORPIO__...` pattern:

- `SCORPIO__ENRICHMENT__ENABLE_TRANSCRIPTS`
- `SCORPIO__ENRICHMENT__ENABLE_CONSENSUS_ESTIMATES`
- `SCORPIO__ENRICHMENT__ENABLE_EVENT_NEWS`
- `SCORPIO__ENRICHMENT__MAX_EVIDENCE_AGE_HOURS`

`ProviderCapabilities` is derived directly from these fields.

### Entity Resolution

New file:

- `src/data/entity.rs`

This module centralizes canonical instrument identity while delegating ticker-format validation to the existing
`src/data/symbol.rs` logic so there is one source of truth for accepted symbol syntax.

Required type:

```rust
pub struct ResolvedInstrument {
    pub input_symbol: String,
    pub canonical_symbol: String,
    pub issuer_name: Option<String>,
    pub exchange: Option<String>,
    pub instrument_type: Option<String>,
    pub aliases: Vec<String>,
}
```

Required function contract:

```rust
pub fn resolve_symbol(symbol: &str) -> Result<ResolvedInstrument, TradingError>;
```

Stage 1 entity-resolution policy:

- empty or format-invalid symbol: return an error and fail preflight
- syntactically valid symbol: canonicalize with uppercase normalization
- valid but metadata-unknown symbol: preserve `canonical_symbol`, leave metadata fields as `None`
- multi-exchange ambiguity: out of scope for Stage 1; leave `exchange` as `None`
- indices, ETFs, and special tickers already accepted by `src/data/symbol.rs`: allow unchanged beyond normalization

### Provider Capabilities

New file:

- `src/data/adapters/mod.rs`

Required type:

```rust
pub struct ProviderCapabilities {
    pub transcripts_enabled: bool,
    pub consensus_estimates_enabled: bool,
    pub event_news_enabled: bool,
}
```

Stage 1 semantics:

- `ProviderCapabilities` means “enabled in runtime config”, not “a concrete provider implementation succeeded”
- capability discovery itself cannot fail in the first slice because it is config-derived only
- future milestones may upgrade this to represent concrete runtime availability

### Provider-Agnostic Enrichment Types

New files:

- `src/data/adapters/transcripts.rs`
- `src/data/adapters/estimates.rs`
- `src/data/adapters/events.rs`

Required normalized output types:

`src/data/adapters/transcripts.rs`

```rust
pub struct TranscriptEvidence {
    pub period_label: Option<String>,
    pub published_at: Option<String>,
    pub speakers: Vec<String>,
    pub key_points: Vec<String>,
}
```

`src/data/adapters/estimates.rs`

```rust
pub struct ConsensusEvidence {
    pub period_label: Option<String>,
    pub revenue_estimate: Option<f64>,
    pub eps_estimate: Option<f64>,
    pub analyst_count: Option<u32>,
    pub published_at: Option<String>,
}
```

`src/data/adapters/events.rs`

```rust
pub struct EventNewsEvidence {
    pub event_type: Option<String>,
    pub headline: String,
    pub published_at: Option<String>,
    pub summary: Option<String>,
    pub relevance_score: Option<f64>,
}
```

Required provider traits:

```rust
#[async_trait]
pub trait TranscriptProvider {
    async fn latest_transcript(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<TranscriptEvidence, TradingError>;
}

#[async_trait]
pub trait EstimatesProvider {
    async fn latest_consensus(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<ConsensusEvidence, TradingError>;
}

#[async_trait]
pub trait EventNewsProvider {
    async fn event_feed(
        &self,
        symbol: &ResolvedInstrument,
    ) -> Result<Vec<EventNewsEvidence>, TradingError>;
}
```

### Machine-Readable Format Rules

For the first implementation slice:

- all timestamps in new Stage 1 types use RFC3339 UTC strings
- coverage ids use only the fixed snake_case identifiers above
- `period_label`, when present, uses the compact form `YYYYQn` such as `2026Q1`

## State Model Changes

### New Stage 1 State Files

Add these files under `src/state/`:

- `evidence.rs`: `EvidenceKind`, `EvidenceRecord<T>`
- `provenance.rs`: `EvidenceSource`, `DataQualityFlag`
- `reporting.rs`: `DataCoverageReport`, `ProvenanceSummary`

`thesis.rs` and `derived.rs` are follow-on files for later milestones and are not part of the first implementation slice.

### Stage 1 Evidence and Provenance Types

Required Stage 1 types:

```rust
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

pub enum DataQualityFlag {
    Missing,
    Stale,
    Partial,
    Estimated,
    Conflicted,
    LowConfidence,
}

pub struct EvidenceRecord<T> {
    pub kind: EvidenceKind,
    pub payload: T,
    pub sources: Vec<EvidenceSource>,
    pub quality_flags: Vec<DataQualityFlag>,
}
```

### Stage 1 Coverage and Reporting Types

Required Stage 1 types:

```rust
pub struct DataCoverageReport {
    pub required_inputs: Vec<String>,
    pub missing_inputs: Vec<String>,
    pub stale_inputs: Vec<String>,
    pub partial_inputs: Vec<String>,
}

pub struct ProvenanceSummary {
    pub providers_used: Vec<String>,
    pub generated_at: String,
    pub caveats: Vec<String>,
}
```

### `TradingState` Extension Strategy

Extend `src/state/trading_state.rs` with:

```rust
pub evidence_fundamental: Option<EvidenceRecord<FundamentalData>>,
pub evidence_technical: Option<EvidenceRecord<TechnicalData>>,
pub evidence_sentiment: Option<EvidenceRecord<SentimentData>>,
pub evidence_news: Option<EvidenceRecord<NewsData>>,
pub data_coverage: Option<DataCoverageReport>,
pub provenance_summary: Option<ProvenanceSummary>,
```

Initialize all new fields to `None` inside `TradingState::new`.

Do not remove or rename these legacy fields in Stage 1:

- `fundamental_metrics`
- `technical_indicators`
- `market_sentiment`
- `macro_news`

### Dual-Write Transition Contract

During the first implementation slice:

- `AnalystSyncTask` writes both legacy analyst fields and new evidence/reporting fields
- newly added report and prompt logic reads the new typed evidence/coverage fields when present
- legacy fields remain compatibility mirrors for older code paths
- if legacy and new fields disagree, treat that as a bug; new typed evidence is authoritative for newly added readers

Coverage authority rule for Stage 1:

- `data_coverage.missing_inputs` is derived from the presence or absence of the new `evidence_*` fields, not from the
  legacy mirrors
- the mapping table above converts each missing `evidence_*` field into the external coverage id

### Stage 1 Quality Detection Rules

Quality detection is intentionally minimal and deterministic in the first slice:

- `required_inputs`: always `fundamentals`, `sentiment`, `news`, `technical`
- `missing_inputs`: derived only from absent `evidence_*` fields using the mapping table above
- `stale_inputs`: always `[]` in Stage 1
- `partial_inputs`: always `[]` in Stage 1
- `DataQualityFlag::Conflicted`: reserved for later milestones and not emitted in Stage 1

`EvidenceRecord<T>.quality_flags` contract for Stage 1:

- always initialize `quality_flags` to `[]`
- do not emit `Missing`, `Partial`, `Estimated`, `Conflicted`, or `LowConfidence` inside `EvidenceRecord<T>` yet
- express Stage 1 quality state only through `DataCoverageReport` and `ProvenanceSummary`

This avoids hidden heuristics while the evidence model is new.

### Analyst vs Sync Responsibility Boundary

The ownership boundary must be explicit:

- analyst tasks own per-analyst data retrieval, per-analyst summaries, and deterministic metrics derived entirely from a
  single analyst dataset
- `AnalystSyncTask` owns cross-source normalization, coverage/provenance aggregation, and any deterministic metrics that
  combine or compare outputs across analysts

Provenance construction ownership for Stage 1:

- `AnalystSyncTask` constructs `EvidenceSource` values for the four baseline analyst outputs using fixed mappings
- it does not ask the LLM to produce provenance fields
- fixed Stage 1 mappings are:
  - fundamentals -> provider `finnhub`, dataset `fundamentals`
  - sentiment -> provider `finnhub`, dataset `company_news_sentiment_inputs`
  - news -> two sources: provider `finnhub`, dataset `company_news` and provider `fred`, dataset `macro_indicators`
  - technical -> provider `yfinance`, dataset `ohlcv`
- `effective_at`, `url`, and `citation` are `None` in the first slice unless an existing adapter already provides the
  value without extra design work
- `providers_used` is sorted ascending and deduplicated for stable reporting and tests
- `required_inputs` and `missing_inputs` keep the fixed coverage-id order: `fundamentals`, `sentiment`, `news`,
  `technical`

## Prompt and Agent Contract Changes

### Shared Prompt Layer

For the first implementation slice, `src/agents/shared/prompt.rs` must provide:

```rust
pub(crate) fn build_authoritative_source_prompt_rule() -> &'static str;
pub(crate) fn build_missing_data_prompt_rule() -> &'static str;
pub(crate) fn build_data_quality_prompt_rule() -> &'static str;
pub(crate) fn build_evidence_context(state: &TradingState) -> String;
pub(crate) fn build_data_quality_context(state: &TradingState) -> String;
```

`build_thesis_memory_context(...)` is deferred to the thesis-memory follow-on milestone.

Required first-slice rendering contract:

`build_evidence_context(state)` renders a compact block like:

```text
Typed evidence snapshot:
- fundamentals: <json or null>
- sentiment: <json or null>
- news: <json or null>
- technical: <json or null>
```

`build_data_quality_context(state)` renders a compact block like:

```text
Data quality snapshot:
- required_inputs: [...]
- missing_inputs: [...]
- providers_used: [...]
```

If the relevant state fields are absent, these builders must render explicit fallback text instead of panicking.

### First-Slice Prompt Rules

Apply these rules across analysts, researchers, risk, trader, and fund manager:

1. prefer authoritative runtime evidence over inference
2. use schema-compatible nulls or empty collections instead of guessing
3. distinguish observed facts from interpretation
4. lower confidence when evidence is missing or sparse
5. surface unresolved uncertainty explicitly when evidence is weak or incomplete

## Report Contract

The first implementation slice adds two new report sections only:

1. `Data Quality and Coverage`
2. `Evidence Provenance`

Minimum contract for `Data Quality and Coverage`:

- heading must be exactly `Data Quality and Coverage`
- list required inputs and missing inputs when data exists
- render exact fallback string `Unavailable` when `data_coverage` is absent

Minimum contract for `Evidence Provenance`:

- heading must be exactly `Evidence Provenance`
- list provider names used for current-run evidence and caveats when data exists
- render exact fallback string `Unavailable` when `provenance_summary` is absent

First-slice placement:

- insert both sections after `Analyst Evidence Snapshot`
- before research and risk summaries

`Scenario Valuation` and `Thesis Status` are follow-on report sections for later milestones.

## Error Handling

Keep the existing `TradingError` in `src/error.rs`.

First-slice rules:

1. `PreflightTask` hard-fails on invalid symbol input or orchestration corruption.
2. Optional enrichment absence or optional enrichment fetch failure does not abort the cycle.
3. Snapshot serialization and deserialization failures remain hard `TradingError::Storage` failures.
4. Prompt contract violations remain `TradingError::SchemaViolation` failures.
5. Report rendering must never panic on missing optional data.

## Testing Strategy

Add tests in this order:

1. config tests for `DataEnrichmentConfig`
2. entity-resolution tests in `src/data/entity.rs`
3. serde tests for new adapter and state types
4. `PreflightTask` tests in `src/workflow/tasks/preflight_tests.rs`
5. context-bridge round-trip tests in `src/workflow/context_bridge.rs`
6. `AnalystSyncTask` dual-write and coverage tests in `src/workflow/tasks/tests.rs`
7. prompt helper and prompt-rendering tests
8. report tests for the new sections and exact fallback strings

Repository-level verification remains:

- `cargo fmt -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo nextest run --all-features --locked`

## Follow-On Milestones

### Milestone 5: Thesis Memory

Add:

- `src/state/thesis.rs`
- `build_thesis_memory_context(state)` in `src/agents/shared/prompt.rs`

Persistence boundary:

- lookup key: `asset_symbol` plus the most recent compatible snapshot lineage
- storage boundary: existing snapshot store unless a dedicated thesis store is introduced later
- missing prior thesis: treat as `None` and continue
- stale or structurally incompatible prior thesis: drop it, log the issue, and continue

### Milestone 6: Peer/Comps and Scenario Valuation

Add:

- `src/state/derived.rs`
- scenario-aware fields in `src/state/proposal.rs`

### Milestone 7: Earnings and Event Enrichment

Implement concrete providers behind the provider-agnostic adapter traits and replace Stage 1 `null` placeholders with
real normalized payloads.

### Milestone 8: Analysis Pack Extraction

Analysis packs are explicitly deferred. Stage 1 should not shape APIs around packs beyond using generic concepts like
coverage ids and provider-agnostic contracts.

## Approved Boundaries

The following decisions are approved for implementation planning:

1. the five-phase business workflow stays intact
2. `PreflightTask` is the only new Stage 1 graph node
3. first-slice required inputs are only `fundamentals`, `sentiment`, `news`, and `technical`
4. provider capabilities in Stage 1 are config-derived only
5. cached enrichment keys use present-with-`null` placeholder semantics
6. typed evidence/coverage fields are the source of truth for newly added readers during the dual-write phase
7. thesis memory, scenario valuation, concrete enrichment providers, and analysis packs are deferred to follow-on plans
8. `README.md` must mention inspiration from `financial-services-plugins`
