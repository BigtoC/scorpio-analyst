---
title: Prompt Bundle Centralization — Runtime Contract Migration
date: 2026-04-25
type: refactor-followup
status: stub-pending-completion
tags:
  - prompt-bundle
  - topology
  - preflight
  - runtime-policy
  - schema-version
  - regression-gate
origin_plan: docs/plans/2026-04-25-001-refactor-prompt-bundle-centralization-plan.md
---

# Prompt Bundle Centralization — Runtime Contract Migration

> **Stub status.** This document is the planned-pattern outline. The
> post-merge follow-up edits it to reflect the actually-shipped pattern,
> including any review-driven changes to Units 4a/4b. **Do not treat the
> contents below as final until the status frontmatter says
> `shipped`.**

## Problem

The runtime had two prompt-ownership paths:
- pack-owned templates under `crates/scorpio-core/src/analysis_packs/*/prompts/*.md`,
  transported through `RuntimePolicy.prompt_bundle`;
- agent-local fallback constants in prompt builders (`TRADER_SYSTEM_PROMPT`,
  `FUND_MANAGER_SYSTEM_PROMPT`, researcher / risk constants).

That duplication left prompt prose writable in two places, spread validation
across render sites instead of enforcing one contract at startup, and let
routing decisions read raw round counters in `workflow::builder` closures
while analyst fan-out was frozen from `pack.required_inputs` before
`PreflightTask` ran.

## Solution

Make `AnalysisPackManifest.prompt_bundle` the only runtime prompt source for
active packs and move prompt-slot enforcement to `PreflightTask` so invalid
active packs fail before any analyst or model task runs.

### Pieces shipped

- `workflow/topology.rs` — single source of truth for `Role`,
  `RunRoleTopology`, `RoutingFlags`, `build_run_topology`,
  `required_prompt_slots`. Role-to-slot mapping is encoded as an exhaustive
  `match` so adding a `Role` variant becomes a compile error until the table
  is extended.
- `analysis_packs::validate_active_pack_completeness` + `CompletenessError`
  — top-level helper near the manifest schema boundary; returns missing
  slots in stable `BTreeSet` order so multi-slot diagnostics are
  deterministic across runs.
- `analysis_packs::init_diagnostics` — single seam invoked from
  `AnalysisRuntime::new` that enumerates registered packs, skips packs
  whose `prompt_bundle.is_empty()` (the existing crypto stub sentinel), and
  emits non-blocking `info!` lines for any active pack incomplete under the
  fully-enabled would-be topology.
- `prompts::validation` — pure helpers: `is_effectively_empty(slot)` (closed
  allowlist of three known placeholder tokens) shared by completeness
  validation and prompt-builder blank-slot guards; `sanitize_analysis_emphasis`
  (strict 0x20–0x7E ASCII, role-injection-tag rejection, 256-char cap) as
  defense-in-depth against pack-author error.
- `agents::risk::DualRiskStatus::StageDisabled` — distinguishes a deliberate
  zero-round bypass from degraded missing-data state. Topology-aware
  constructor `from_reports_with_topology(c, n, risk_stage_enabled)` is the
  intended entry point.
- `tests/prompt_bundle_regression_gate.rs` + `tests/fixtures/prompt_bundle/`
  — golden-byte fixtures locking the rendered baseline templates after
  canonical placeholder substitution. The merge gate for the runtime-contract
  migration: signature changes are allowed; rendered bytes are not.
- `tests/second_consumer_abstraction.rs` — R8 API-shape contract test that
  constructs a synthetic non-baseline `AnalysisPackManifest` in test code
  and asserts the topology functions accept a non-baseline shape.
- `THESIS_MEMORY_SCHEMA_VERSION` `2 -> 3` (Unit 4b — pending) — read-side
  skip semantics; **no destructive migration**.

### What this refactor deliberately did *not* do

- No new selectable packs.
- No new ticker validator. `{ticker}` continues to flow through the existing
  `validate_symbol` syntactic gate + data-API existence chain. Adding a
  regex allowlist would create a third validator and a breaking change for
  legitimate dotted/longer-suffix tickers (`BRK.B`, `RY.TO`, `0700.HK`).
- No empirical validation of the topology abstraction against a real
  second pack. The Unit 5 fixture test verifies API shape; **real-world
  fitness is deferred** until the first real second pack lands. Future
  asset-class work begins from "we shipped the API-shape test; real-world
  fitness is yet to be proven."

## Why active-pack completeness lives in preflight, not manifest validation

`AnalysisPackManifest::validate()` is shape-only and runs against every
registered pack including the inactive crypto stub (`PromptBundle::empty()`).
Promoting active completeness into that boundary would force every stub
pack to ship full prompt assets just to register, which contradicts R6.
`PreflightTask` is the per-cycle authority that knows the active pack and
the configured round counts; that is the right place to enforce
"every required slot for *this* run is populated."

## Why the regression gate uses golden-byte fixtures, not in-memory snapshots

A characterization test authored in the same PR as the topology mapping
inevitably tests what the author already imagined. The on-disk golden bytes
travel across the Unit 4a/4b API change: the harness code is rewritten when
the prompt-builder signature flips to require `&RuntimePolicy`, but the
expected bytes stay the same. The merge gate is byte-equality, not
harness-equality.

## Schema-version operational notes

`THESIS_MEMORY_SCHEMA_VERSION` `2 -> 3` is non-destructive. The existing
read-side skip path at `thesis.rs:83` (`if schema_version != THESIS_MEMORY_SCHEMA_VERSION`)
silently ignores rows in either direction; v2 rows persist on disk and are
ignored on read. The deserialization-failure `warn!` was updated to drop the
`%err` field (which can echo payload-shaped substrings from `serde_json`)
and emit `error.kind = "deserialize"` instead. Operators may purge stale
rows manually with `DELETE FROM phase_snapshots WHERE schema_version < 3`.

A future `scorpio db vacuum` subcommand (or TTL-based cleanup) is the right
home for automated reclamation; this refactor does not introduce one.

## Open follow-ups

- **Unit 4a Step 3**: prompt-builder signature flip across ~13 call sites —
  pending. Behavior-neutral so far thanks to the regression gate.
- **Unit 4b**: routing flip via `RoutingFlags` reads, maximal-children
  fan-out + per-child no-op gating, `try_new` + production-caller routing,
  schema bump, `sanitize_analysis_emphasis` enforcement, fallback constant
  deletion — pending.
- **Unit 6 cleanup**: `README.md` / `CLAUDE.md` refresh + dead-constant
  deletion — pending until 4b removes the fallback constants from the
  active runtime path.
