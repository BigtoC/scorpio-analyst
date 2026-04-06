# `evidence-provenance` Capability

## ADDED Requirements

### Requirement: Final Report Shows Data Quality And Coverage

The human-readable terminal final report MUST always render a `Data Quality and Coverage` section immediately after
`Analyst Evidence Snapshot` and before `Evidence Provenance` and `Research Debate Summary`.

When `TradingState.data_coverage` is `Some(DataCoverageReport)`, the section MUST list the required analyst inputs
explicitly and surface any missing, stale, or partial inputs in a human-readable form. When all three issue lists are
empty, the section MUST still make clear that all required inputs are present.

When `TradingState.data_coverage` is `None`, the section MUST still appear and render the exact fallback string
`Unavailable`.

#### Scenario: Coverage Section Appears In Final Report Order

- **WHEN** `format_final_report` renders a completed `TradingState`
- **THEN** `Analyst Evidence Snapshot` is followed by `Data Quality and Coverage`, then `Evidence Provenance`, then
  `Research Debate Summary`

#### Scenario: Coverage Section Lists Required Inputs And Shows Missing And Partial Inputs

- **WHEN** `TradingState.data_coverage` contains `required_inputs`, `missing_inputs`, `stale_inputs`, and
  `partial_inputs`
- **THEN** the report includes the `Data Quality and Coverage` heading, lists the required input names, and renders
  labeled issue lists for each non-empty category

#### Scenario: Coverage Section Falls Back To Unavailable

- **WHEN** `TradingState.data_coverage` is `None`
- **THEN** the report still includes the `Data Quality and Coverage` heading and renders the exact string
  `Unavailable`

### Requirement: Final Report Shows Evidence Provenance

The human-readable terminal final report MUST always render an `Evidence Provenance` section immediately after
`Data Quality and Coverage` and before `Research Debate Summary`.

When `TradingState.provenance_summary` is `Some(ProvenanceSummary)`, the section MUST list the providers that
contributed evidence and any caveats attached to the summary. If either list is empty, the section MUST render an
explicit none-style label rather than omitting the line entirely.

When `TradingState.provenance_summary` is `None`, the section MUST still appear and render the exact fallback string
`Unavailable`.

#### Scenario: Provenance Section Shows Providers And Caveats

- **WHEN** `TradingState.provenance_summary` contains provider names and caveats
- **THEN** the report includes the `Evidence Provenance` heading, a labeled provider list, and a labeled caveat list

#### Scenario: Provenance Section Shows Explicit None Labels

- **WHEN** `TradingState.provenance_summary` is present but both `providers_used` and `caveats` are empty
- **THEN** the report renders explicit none-style labels for both providers and caveats rather than omitting them

#### Scenario: Provenance Section Falls Back To Unavailable

- **WHEN** `TradingState.provenance_summary` is `None`
- **THEN** the report still includes the `Evidence Provenance` heading and renders the exact string `Unavailable`
