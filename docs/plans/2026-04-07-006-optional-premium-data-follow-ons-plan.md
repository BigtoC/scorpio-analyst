---
title: feat: Optional premium-data valuation and enrichment follow-ons
type: feat
status: optional
date: 2026-04-07
---

# Optional Premium-Data Valuation And Enrichment Follow-Ons

## Overview

Capture the valuation and enrichment capabilities intentionally deferred from the active roadmap because the current implementation track is limited to free-tier Finnhub, yfinance, and FRED.

This is an optional follow-on plan. It should only be considered after the provider-limited roadmap is complete and the project has access to stronger data sources or a clear business reason to add premium integrations.

## Why This Exists

The active roadmap now intentionally ships a bounded system:

- thesis memory on top of snapshots
- deterministic valuation only for supported input shapes
- event/news enrichment first
- optional or deferred estimates support depending on provider verification
- no transcript delivery in the active slice

The original roadmap also pointed toward richer capabilities that remain valuable, but they are not realistic with the current provider set:

- earnings call transcripts
- reliable consensus estimates if free-tier verification fails
- peer/comps datasets and sector median multiples
- historical valuation bands
- `P/S`, `PEG`, `EV/EBITDA`, and DCF-style valuation inputs
- ETF-native valuation inputs such as NAV / premium-discount, fund flows, and holdings-based analytics

This plan keeps those ideas visible without distorting the active implementation sequence.

## Requirements Trace

- R1. Add transcript enrichment behind the existing transcript adapter contract.
- R2. Add any still-missing consensus-estimates enrichment behind the estimates adapter contract.
- R3. Add richer typed valuation inputs needed for `P/S`, `PEG`, `EV/EBITDA`, and DCF-style deterministic valuation.
- R4. Add typed peer/comps and sector/industry baseline data if deterministic peer-relative valuation is still desired.
- R5. Add ETF-native valuation inputs if ETF-specific deterministic assessment becomes a product requirement.
- R6. Keep the provider-limited roadmap backward-compatible.

## Scope Boundaries

- No changes required for the active provider-limited roadmap to ship first.
- No requirement to implement every deferred capability together.
- No promise that premium-data follow-ons will remain single-vendor; this plan may require multiple providers.

## Deferred Capability Buckets

### 1. Transcript enrichment

- Add concrete transcript fetching behind `src/data/adapters/transcripts.rs`.
- Update prompt/report consumers to surface transcript-backed evidence.
- Keep fail-open semantics if transcript access is rate-limited or unavailable.

### 2. Rich issuer estimates / valuation inputs

- Add missing estimates fields if free-tier Finnhub proves insufficient.
- Add typed inputs for valuation models currently referenced in prompts but unsupported in the active slice:
  - revenue / sales inputs for `P/S`
  - enterprise value
  - EBITDA
  - free cash flow
  - discount rate
  - terminal growth
  - forecast horizon
  - shares outstanding / market cap normalization

### 3. Peer/comps and historical baselines

- Add sector or industry median multiples.
- Add peer/comps datasets and selection rules.
- Add historical valuation bands if they materially improve deterministic valuation quality.

### 4. ETF-native valuation inputs

- Add typed ETF-specific inputs such as:
  - NAV / premium-discount
  - expense ratio
  - AUM
  - holdings concentration
  - benchmark / index metadata
  - fund flows
- Add an ETF-native deterministic assessment path rather than treating ETFs as unsupported asset shapes.

## Suggested Activation Criteria

Revisit this optional plan only when one or more of the following becomes true:

- the active provider-limited roadmap is complete and stable
- premium or alternative data providers are available
- transcript-backed analysis becomes a product requirement
- deterministic valuation quality becomes a bigger bottleneck than general evidence quality
- ETF-native valuation becomes a first-class product goal

## Relationship To Active Plans

- Builds on `docs/plans/2026-04-07-002-feat-peer-comps-scenario-valuation-plan.md`
- Builds on `docs/plans/2026-04-07-003-feat-concrete-enrichment-providers-plan.md`
- May later extend `docs/plans/2026-04-07-004-feat-analysis-pack-extraction-plan.md` with richer pack vocabularies once these inputs exist

## Documentation / Operational Notes

- Treat this plan as optional backlog, not part of the current implementation commitment.
- Link future premium-provider evaluations back to this plan rather than reopening the active roadmap docs.
