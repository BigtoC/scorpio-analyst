---
title: feat: Optional premium-data valuation and enrichment follow-ons
type: feat
status: optional
date: 2026-04-07
---

# Optional Premium-Data Valuation And Enrichment Follow-Ons

## Overview

This plan is an optional follow-on that has been extremely narrowed because we successfully achieved institutional-grade valuation on the free tier. The scope now only covers capabilities that are strictly gated behind premium data sources or require significant additional cost to acquire.

This plan should only be considered after the core roadmap is complete and the project has access to stronger data sources or a clear business reason to add premium integrations.

## Why This Exists

We successfully unlocked DCF, EV/EBITDA, Forward P/E, PEG, P/S, historical valuation bands, and Consensus Estimates on the free tier. As a result, the active roadmap now supports highly sophisticated deterministic valuation.

However, a few extremely rich capabilities remain inaccessible without premium data providers:

- Earnings Call Transcripts
- Sector Peer/Comps datasets and medians
- Deep ETF-native metrics (NAV, Holdings Weights)

This plan keeps those remaining premium ideas visible without distorting the active implementation sequence.

## Requirements Trace

- R1. Add transcript enrichment behind the existing transcript adapter contract.
- R2. Add typed peer/comps and sector/industry baseline data.
- R3. Add deep ETF-native valuation inputs if ETF-specific deterministic assessment becomes a product requirement.
- R4. Keep the provider-limited roadmap backward-compatible.

## Scope Boundaries

- No changes required for the active roadmap to ship first.
- No requirement to implement every deferred capability together.
- No promise that premium-data follow-ons will remain single-vendor; this plan may require multiple providers.

## Deferred Capability Buckets

### 1. Earnings Call Transcripts

- Add concrete transcript fetching behind `src/data/adapters/transcripts.rs`.
- Update prompt/report consumers to surface transcript-backed evidence.
- Keep fail-open semantics if transcript access is rate-limited or unavailable.

### 2. Sector Peer/Comps and Baselines

- Add sector or industry median multiples.
- Add peer/comps datasets and selection rules.

### 3. Deep ETF-native valuation inputs

- Add typed ETF-specific inputs such as:
  - NAV / premium-discount
  - Holdings Weights / concentration
  - expense ratio
  - AUM
  - benchmark / index metadata
  - fund flows
- Add an ETF-native deterministic assessment path rather than treating ETFs as generic asset shapes.

## Suggested Activation Criteria

Revisit this optional plan only when one or more of the following becomes true:

- the active roadmap is complete and stable
- premium or alternative data providers are available and budgeted
- transcript-backed analysis becomes a product requirement
- ETF-native valuation becomes a first-class product goal

## Relationship To Active Plans

- Builds on `docs/plans/2026-04-07-002-feat-peer-comps-scenario-valuation-plan.md`
- Builds on `docs/plans/2026-04-07-003-feat-concrete-enrichment-providers-plan.md`
- May later extend `docs/plans/2026-04-07-004-feat-analysis-pack-extraction-plan.md` with richer pack vocabularies once these inputs exist

## Documentation / Operational Notes

- Treat this plan as optional backlog, not part of the current implementation commitment.
- Link future premium-provider evaluations back to this plan rather than reopening the active roadmap docs.
