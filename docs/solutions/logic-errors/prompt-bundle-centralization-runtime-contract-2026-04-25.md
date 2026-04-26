---
title: Prompt Bundle Centralization — Runtime Contract Migration
date: 2026-04-26
last_updated: 2026-04-26
type: refactor-followup
status: shipped
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

This document captures the **shipped** pattern after Units 1-6 of the
prompt-bundle centralization refactor landed. The runtime contract below is
authoritative; the origin plan is preserved as a historical record of how
the work was scoped and reviewed.

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

## Shipped follow-ups (Phases 1-10 of the plan)

All originally-deferred work landed:

- **Phase 1**: Regression gate at `tests/prompt_bundle_regression_gate.rs`
  drives the `testing::prompt_render` harness across 13 roles × 4
  scenarios. Golden-byte fixtures live under `tests/fixtures/prompt_bundle/`.
- **Phase 2**: `THESIS_MEMORY_SCHEMA_VERSION 2 → 3`; warn-line replaced
  `%err` with `error.kind = "deserialize"` so `serde_json` errors cannot
  echo payload bytes into logs. Bidirectional thesis-compat tests (v3 binary
  skips v2 rows; v2 binary skips v3 rows after downgrade).
- **Phase 3**: `DualRiskStatus::from_reports_with_topology` wired into
  `fund_manager::agent`. The `risk_stage_enabled_for_state` helper currently
  always returns `true` (preserving today's `Unknown`-on-missing-reports
  semantic); future work plumbs `KEY_ROUTING_FLAGS` from `FundManagerTask`
  to make the `StageDisabled` variant reachable in production.
- **Phase 4**: `PreflightTask` runs `validate_active_pack_completeness` as
  fail-loud and invokes `sanitize_analysis_emphasis` against the active
  pack's emphasis string. The previously-passing `from_pack` test for the
  inactive crypto stub was updated to assert the new fail-loud contract:
  construction succeeds, but preflight rejects the empty bundle.
- **Phase 5**: `TradingPipeline::try_new` introduced alongside the
  infallible `::new`. `AnalysisRuntime` routes through `try_new` so an
  invalid `config.analysis_pack` value surfaces as `TradingError::Config`
  at construction time.
- **Phase 6**: Activation-path audit at `tests/activation_path_audit.rs`
  proves every reachable construction path (`new`, `try_new`, `from_pack`,
  `build_graph_from_pack`) produces a graph whose entry task is
  `PreflightTask`.
- **Phase 7**: `render_researcher_system_prompt` and
  `render_risk_system_prompt` now take `&RuntimePolicy` directly with no
  legacy-template fallback. Each agent constructor extracts the policy from
  `state.analysis_runtime_policy` via the per-module
  `runtime_policy_for_agent` helper, returning a typed `Config` error if
  the policy is missing (preflight-bypass without `with_baseline_runtime_policy`).
- **Phase 8**: `state.analysis_runtime_policy` reset confirmed to be
  hygiene-only — preflight is the sole writer in production.
- **Phase 9**: Stage-entry conditional edges in `workflow::builder` read
  `RoutingFlags` (typed) from `KEY_ROUTING_FLAGS`. Loop-back conditionals
  keep using the per-iteration round counters per the plan. Fallback to the
  raw round-count read preserves test compatibility for paths that
  legitimately bypass preflight.
- **Phase 10**: Legacy `_SYSTEM_PROMPT` constants retained as
  `#[allow(dead_code)]` drift-detection oracles (the byte-equivalence tests
  in `agents/researcher/common.rs` and `agents/risk/common.rs` still
  compare them to the rendered pack assets). Vacuous fallback test helpers
  (`render_legacy_fallback_system_prompt_for_role`,
  `render_blank_slot_fallback_system_prompt_for_role`) and their tests
  removed because the renderer paths they exercised are gone. README /
  CLAUDE.md refreshed.

The deferred items called out in earlier drafts of this document
(real-world abstraction validation against a real second pack;
`scorpio db vacuum` for thesis-row reclamation) remain deferred to the
slice that ships a second selectable pack.

## Review-driven hardening after shipment

A follow-up review pass on the same branch closed the remaining drift between
the shipped runtime contract and the prompt/test surfaces:

- `testing::runtime_policy::runtime_policy_from_manifest` was added as the
  shared test-only manifest-to-policy seam. Integration tests that exercise
  synthetic manifests (`tests/second_consumer_abstraction.rs` and
  `tests/prompt_bundle_regression_gate.rs`) now validate completeness against
  the same `RuntimePolicy` boundary production uses instead of reconstructing
  local setup around `AnalysisPackManifest`.
- `tests/second_consumer_abstraction.rs` is now gated behind
  `#![cfg(feature = "test-helpers")]` so its helper usage matches the repo's
  integration-test contract.
- Duplicate local `with_baseline_runtime_policy` helpers in
  `agents/trader/tests.rs` and `agents/fund_manager/tests.rs` were replaced by
  the shared testing helper, keeping one test-only runtime-policy contract.
- Trader and fund-manager prompt builders finished the R1/R5 ownership move:
  absent upstream inputs now serialize as structured runtime values (`null`,
  `Upstream data state: complete|incomplete`, and
  `Dual-risk escalation: stage_disabled|...`) while the interpretation rules
  live in the pack-owned assets under
  `analysis_packs/equity/prompts/trader.md` and `fund_manager.md`.
- `testing::prompt_render` now maps the zero-risk fixture path to
  `DualRiskStatus::StageDisabled`, and the prompt-bundle golden fixtures were
  refreshed to lock the updated user-visible contract.
- `prompts/mod.rs` was corrected to describe the actual runtime state of the
  world: active prompt prose is pack-owned and builders are mechanical
  renderers over runtime policy and state.

This hardening pass was re-verified with the full repo commands:

- `cargo fmt -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace --all-features --locked --no-fail-fast`
