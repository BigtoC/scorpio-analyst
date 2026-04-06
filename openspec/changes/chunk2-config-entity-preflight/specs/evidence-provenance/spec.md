# `evidence-provenance` Capability

## ADDED Requirements

### Requirement: Runtime Preflight Resolves Canonical Instrument Identity

Before any analyst task runs, the workflow MUST perform a preflight step that validates and canonicalizes the runtime
asset symbol into a `ResolvedInstrument`.

The preflight step MUST:

- read the symbol from the runtime `TradingState`
- validate it through the shared symbol-validation contract
- canonicalize accepted symbols to uppercase
- write the canonical symbol back into `TradingState.asset_symbol`
- serialize a `ResolvedInstrument` into workflow context under `KEY_RESOLVED_INSTRUMENT`

In Stage 1, `ResolvedInstrument.issuer_name`, `exchange`, and `instrument_type` remain `None`, and `aliases` remains
empty unless later enrichment work populates them.

#### Scenario: Lowercase Symbol Is Canonicalized Before Analyst Execution

- **WHEN** `TradingState.asset_symbol` is `"nvda"`
- **THEN** preflight succeeds, writes a `ResolvedInstrument` with `canonical_symbol = "NVDA"`, and updates
  `TradingState.asset_symbol` to `"NVDA"` before analyst fan-out begins

#### Scenario: Invalid Symbol Fails Closed

- **WHEN** `TradingState.asset_symbol` contains an invalid value such as `"DROP;TABLE"`
- **THEN** preflight returns an error and the pipeline does not dispatch any analyst task

### Requirement: Stage 1 Enrichment Capability Contracts Are Declared

The system MUST declare the Stage 1 enrichment capability seam in `src/data/adapters/`.

This seam MUST include:

- `ProviderCapabilities` derived entirely from `DataEnrichmentConfig`
- `TranscriptEvidence` and `TranscriptProvider`
- `ConsensusEvidence` and `EstimatesProvider`
- `EventNewsEvidence` and `EventNewsProvider`

In this slice, these are contract-only types and traits. No concrete transcript, estimates, or event-news provider
implementations are required.

#### Scenario: Capability Flags Reflect Config Only

- **WHEN** `DataEnrichmentConfig` enables transcripts and disables the other enrichment categories
- **THEN** `ProviderCapabilities::from_config(...)` produces a struct with only the transcript capability enabled,
  without performing any runtime provider discovery call

### Requirement: Preflight Seeds Required Coverage Inputs And Typed Null Placeholders

The preflight step MUST write the Stage 1 evidence/provenance context contract into workflow context before analyst
fan-out begins.

It MUST write:

- `KEY_PROVIDER_CAPABILITIES`
- `KEY_REQUIRED_COVERAGE_INPUTS`
- `KEY_CACHED_TRANSCRIPT`
- `KEY_CACHED_CONSENSUS`
- `KEY_CACHED_EVENT_FEED`

`KEY_REQUIRED_COVERAGE_INPUTS` MUST preserve this exact ordered value:

- `fundamentals`
- `sentiment`
- `news`
- `technical`

The three `KEY_CACHED_*` entries MUST be present even when no enrichment data exists yet, and their Stage 1 value MUST
be a typed JSON `null` placeholder rather than an absent key.

#### Scenario: Coverage Input Baseline Is Present In Fixed Order

- **WHEN** preflight succeeds
- **THEN** `KEY_REQUIRED_COVERAGE_INPUTS` is present in workflow context with the ordered value
  `["fundamentals", "sentiment", "news", "technical"]`

#### Scenario: Enrichment Cache Keys Use Present-With-Null Semantics

- **WHEN** preflight succeeds before any transcript, consensus, or event-news provider has populated data
- **THEN** `KEY_CACHED_TRANSCRIPT`, `KEY_CACHED_CONSENSUS`, and `KEY_CACHED_EVENT_FEED` are all present in workflow
  context and each contains a typed JSON `null` placeholder
