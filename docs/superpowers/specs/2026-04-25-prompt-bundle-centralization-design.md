# Prompt Bundle Centralization Design

**Date:** 2026-04-25
**Status:** Approved

## Goal

Remove prompt prose duplication by making `AnalysisPackManifest.prompt_bundle` the only runtime prompt source for active packs. Agents should load the prompt for their role from the selected pack, and an active pack with a missing required prompt slot should fail in preflight before any agent runs.

This change ships under the asset-class-generalization track because the topology-driven required-slot derivation is what lets future non-equity packs (e.g. the existing crypto stub) activate without inheriting equity analyst prompts they never run. Without that derivation step, every new asset-class pack would either need to populate equity-only slots or special-case preflight; with it, a new pack just declares its own role roster and the rest follows.

## Problem

The current runtime has two prompt ownership paths:

- pack-owned prompt assets under `analysis_packs/.../prompts/*.md`, carried through `prompt_bundle`
- agent-local prompt constants such as `FUNDAMENTAL_SYSTEM_PROMPT`, `TRADER_SYSTEM_PROMPT`, and `FUND_MANAGER_SYSTEM_PROMPT`

At runtime the code prefers `state.analysis_runtime_policy.prompt_bundle.<slot>` and falls back to the agent-local constant when the slot is empty. That keeps stub packs working, but it leaves canonical prompt prose duplicated across two writable surfaces and weakens the architecture:

- the selected pack is not the single source of truth for prompt prose
- prompt correctness is deferred to individual render sites instead of being validated once up front
- new packs can appear valid while depending on unrelated fallback text owned by agent modules
- the same prose drifts independently in two places under PR pressure

The desired architecture is simpler: the active pack owns the prompt prose, preflight validates that ownership, and agents only consume the validated slot assigned to their role.

## Decisions

### Prompt Ownership

| Decision                | Choice                                                                          | Rationale                                                                                   |
|-------------------------|---------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------|
| Prompt source of truth  | `AnalysisPackManifest.prompt_bundle` only                                       | Removes duplicated runtime prose                                                            |
| Code-owned prompt prose | Enforced by structural test (rendered prompt == pack asset modulo placeholders) | "Should stay mechanical" without enforcement is the failure mode the refactor exists to fix |
| Prompt rendering        | Keep placeholder substitution in agent prompt builders                          | Prompt text stays pack-owned; runtime values stay agent-owned                               |
| Agent fallback logic    | Removed                                                                         | Keeps agents focused on rendering, not prose ownership                                      |

### Validation & Enforcement

| Decision                                 | Choice                                                            | Rationale                                                                           |
|------------------------------------------|-------------------------------------------------------------------|-------------------------------------------------------------------------------------|
| Failure boundary (orchestrated runs)     | Preflight (primary)                                               | Fail before fan-out and before any model call                                       |
| Failure boundary (alternate entrypoints) | Prompt-builder signature requires `&RuntimePolicy` (compile-time) | Tests/replay/backtest paths get a hydration requirement enforced by the type system |
| Manifest/runtime-policy resolution scope | Stays shape-only                                                  | Inactive stub packs must still resolve into a `RuntimePolicy`                       |
| Active-pack-only enforcement             | Hard fail in preflight; non-blocking `info!` warn at registration | Stubs can ship; activation isn't a cliff (contributors get a progressive signal)    |
| Missing runtime policy at prompt builder | Structurally unreachable (required parameter)                     | "Orchestration corruption" becomes a compile error inside the builder               |
| Blank slot at prompt builder             | Typed `TradingError::Config { pack_id, slot_name }`               | Defense-in-depth for any non-preflight caller; surfaces a real bug loudly           |

### Topology & Routing

| Decision                  | Choice                                                                                                                                                               | Rationale                                                                                                                                                      |
|---------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Topology owner            | One typed per-run `RunRoleTopology` produced by `build_run_topology(...)`                                                                                            | Single role-enablement source; no second preflight-only mirror                                                                                                 |
| Two consumers, one source | `enabled_roles_for_routing(&topology) -> RoutingFlags` and `required_prompt_slots(&topology) -> Vec<PromptSlot>` are pure functions over the same shared `Role` enum | Routing and prompt-slot validation share the role enum but don't conflate consumers; new providers/instruments add inputs to the topology, not to a god-struct |
| Topology lifecycle        | Cycle-scoped; computed at preflight; **not** part of `TradingState`                                                                                                  | Resumed runs always re-run preflight; stale topology cannot leak across cycles                                                                                 |
| Routing input             | Conditional edges read `RoutingFlags` from context (written by preflight), not raw config                                                                            | Preflight becomes the *origin* of routing decisions, not just a validator                                                                                      |

### Runtime Semantics Changes

These are the runtime contract changes bundled into this design. They are visible to downstream consumers and must be reviewed as runtime semantics, not just internal refactoring.

| Decision                       | Choice                                                                                                                         | Rationale                                                                                                                                  |
|--------------------------------|--------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------|
| Zero-round stage routing       | Bypass moderator tasks entirely instead of visiting once                                                                       | Required for moderator prompt slots to be optional on the active pack                                                                      |
| Zero-round artifacts           | Skipped-stage artifacts left absent (no synthesized placeholder)                                                               | True-absence semantics are explicit; downstream consumers branch on `Option`                                                               |
| Stage-disabled vs missing-data | New `DualRiskStatus::StageDisabled` variant distinct from `Unknown`                                                            | Prevents zero-risk runs from being conflated with degraded missing-data state in FM rationale                                              |
| Code-owned absence prose       | Trader `MISSING_CONSENSUS_NOTE`, Trader `data_quality_note` strings, FM risk-report placeholder copy all move into pack assets | Substantive prose; not "mechanical sanitization." Enumerated in Component Changes                                                          |
| Snapshot replay compatibility  | `THESIS_MEMORY_SCHEMA_VERSION` bumped from 2 to 3                                                                              | Pre-migration snapshots contain synthesized zero-round artifacts the new code treats as absent; bump retires those rows from thesis lookup |

### Stub Pack Support

| Decision          | Choice                                                                           | Rationale                                                                                                      |
|-------------------|----------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------|
| Stub pack support | Incomplete bundles allowed while pack is inactive                                | Preserves current crypto-pack scaffolding workflow                                                             |
| Activation paths  | All paths from `PackId`/string to a runnable graph must traverse `PreflightTask` | Preflight is the only fence; activation audit is a PR-merge gate                                               |

## Architecture

The architecture becomes:

1. A pack manifest declares its `prompt_bundle`.
2. `resolve_runtime_policy_for_manifest(...)` copies that bundle into `RuntimePolicy` (shape-only — does not check completeness).
3. `run_analysis_cycle` no longer pre-writes `state.analysis_runtime_policy`; that becomes `PreflightTask`'s sole responsibility.
4. `PreflightTask` calls `build_run_topology(&inputs)` to produce a `RunRoleTopology` for the run.
5. `PreflightTask` calls `required_prompt_slots(&topology)` and `validate_active_pack_completeness(...)` against the active `RuntimePolicy.prompt_bundle`. Missing slots fail the task with a typed error containing the active pack id and all missing slot names.
6. `PreflightTask` writes the validated `RuntimePolicy` and the `RoutingFlags` derived from `(&topology).into()` into context. Routing edges read `RoutingFlags`, not raw config.
7. `PreflightTask` does **not** persist `RunRoleTopology` itself — it is cycle-scoped and not added to `TradingState`. A resumed run re-runs preflight, recomputing the topology fresh against current config.
8. Agent prompt builders take `&RuntimePolicy` as a **required parameter** (not read from state). The "missing policy" case is a compile error, not a runtime check.
9. Agents render placeholders such as `{ticker}`, `{current_date}`, `{analysis_emphasis}`, and pack-defined absence-note placeholders, applying only substitution and sanitization mechanics. A regression test (`rendered prompt == pack asset modulo declared placeholders`) is the structural enforcement of the "mechanical only" rule.

### `RunRoleTopology` Interface

```rust
// crates/scorpio-core/src/workflow/topology.rs (new module)

pub struct RunRoleTopology {
    pub analyst_roles: Vec<AnalystRole>,    // derived from pack required_inputs + registry filtering
    pub debate_enabled: bool,                // == max_debate_rounds > 0
    pub risk_enabled: bool,                  // == max_risk_rounds > 0
}

pub struct RoutingFlags {                    // topology-derived, written to context for routing edges
    pub debate_enabled: bool,
    pub risk_enabled: bool,
    pub analyst_roles: Vec<AnalystRole>,
}

impl From<&RunRoleTopology> for RoutingFlags { /* ... */ }

pub enum Role {                              // single shared enum; owns role-to-prompt-slot mapping
    Analyst(AnalystRole),
    BullishResearcher,
    BearishResearcher,
    DebateModerator,
    Trader,
    AggressiveRisk,
    ConservativeRisk,
    NeutralRisk,
    RiskModerator,
    FundManager,
}

impl Role {
    pub fn prompt_slot(&self) -> PromptSlot { /* one match arm per variant */ }
}

pub fn build_run_topology(
    manifest: &AnalysisPackManifest,
    config: &RuntimeConfig,
    registry: &AnalystRegistry,
) -> RunRoleTopology { /* sole constructor */ }

pub fn required_prompt_slots(topology: &RunRoleTopology) -> Vec<PromptSlot> { /* pure */ }
```

`build_run_topology` is the sole function that constructs a `RunRoleTopology`. Future inputs (provider capabilities, instrument metadata) get added to its parameter list, not to call sites — that is the point of the abstraction. `required_prompt_slots` and `RoutingFlags::from(...)` are independent pure derivations that share inputs, not a single owning struct that conflates them.

### Routing/Preflight Ordering

Today's graph wires conditional edges from `KEY_MAX_DEBATE_ROUNDS` / `KEY_MAX_RISK_ROUNDS` directly at construction time, so routing is decided independently of preflight. After this change:

- Graph construction in `build_graph_from_pack` no longer reads round counts. The conditional-edge closures read `RoutingFlags` from context.
- `PreflightTask` runs first (no change to ordering). It writes `RoutingFlags` before any other task can read it.
- Zero-round bypass is implemented by `RoutingFlags::debate_enabled == false` causing the analyst-sync edge to skip the debate moderator and route directly to the trader. Same for risk.
- Analyst fan-out construction reads `RoutingFlags::analyst_roles` (sourced from the topology), not the pack manifest directly. This keeps analyst spawning, debate routing, and risk routing all reading from the same single source.

This makes preflight the *origin* of routing decisions instead of just the *validator* of them, which resolves the otherwise-impossible "one source of truth, but routing is wired before preflight runs" ordering inversion.

### Snapshot Replay & Cycle Reuse

- `RunRoleTopology` is cycle-scoped; it is **not** serialized into `TradingState` and not persisted in `phase_snapshots`.
- A resumed run from any snapshot re-enters the graph at preflight, which recomputes the topology from current `RuntimePolicy` and round-count config.
- If the resumed run's config differs from the snapshot's config, preflight produces a topology consistent with the *current* config. This is deliberate: stale topologies cannot leak across resumes.
- `THESIS_MEMORY_SCHEMA_VERSION` is bumped from 2 to 3. Pre-migration snapshots contain synthesized `consensus_summary` / risk reports for zero-round runs that the new code treats as absent; the version bump retires those rows from thesis lookup rather than allowing them to surface as "real" prior thesis.

### Topology-Derived Keys

`RoutingFlags` is the only topology-derived persistent surface in context. It is exactly `(&RunRoleTopology).into()` — not a separate computation, just a serializable projection that routing edges can read without holding a reference to the topology itself.

`AnalysisPackManifest::validate()` and `resolve_runtime_policy_for_manifest(...)` remain shape-only. They must not enforce active-run prompt completeness, because that would break the accepted requirement that incomplete stub packs may still resolve while inactive. Active-pack completeness lives in `validate_active_pack_completeness` (a separate top-level helper, not on `validate()`), called from `PreflightTask` and from registry-time `info!` diagnostics.

## Required Prompt Slots

The active pack must provide every prompt slot needed by the currently runnable roles, as determined by `required_prompt_slots(&topology)`.

`required_prompt_slots` yields:

- always required (topology-fixed):
  - `trader`
  - `fund_manager`
- analyst slots, one per `AnalystRole` in `topology.analyst_roles` — derived from the same `required_inputs`-plus-registry-filtering path that `build_analyst_tasks` uses for analyst task spawning
- additionally required when `topology.debate_enabled`:
  - `bullish_researcher`
  - `bearish_researcher`
  - `debate_moderator`
- additionally required when `topology.risk_enabled`:
  - `aggressive_risk`
  - `conservative_risk`
  - `neutral_risk`
  - `risk_moderator`

This keeps zero-round workflows valid without forcing unused prompt slots on the active pack, and prevents future non-equity packs from failing preflight on analyst prompts they never run.

When a stage is bypassed, downstream artifacts remain absent rather than synthesized:

- zero-debate runs leave `consensus_summary` absent
- zero-risk runs leave the three risk reports absent and `risk_discussion_history` empty

For these runs to render coherent prompts, the spec relocates several code-owned absence-prose strings into pack assets (see Component Changes / Code-Owned Absence Prose).

For the baseline equity pack, all of the above slots must be populated from extracted prompt assets. For incomplete stub packs such as the current crypto placeholder, empty or partial bundles remain acceptable only while the pack is inactive.

## Component Changes

### `crates/scorpio-core/src/analysis_packs/manifest/schema.rs`

- Keep `prompt_bundle` on `AnalysisPackManifest` as the canonical prompt container.
- `AnalysisPackManifest::validate()` stays shape-only — no completeness checks.
- Add a separate top-level helper `validate_active_pack_completeness(manifest: &AnalysisPackManifest, topology: &RunRoleTopology) -> Result<(), MissingSlots>`. This is **not** a method on `validate()`. It is called from `PreflightTask` (hard fail) and from registry-time registration (non-blocking `info!`).
- Inactive stub packs continue to register; the registration-time call to `validate_active_pack_completeness` against the stub's own would-be topology emits a `tracing::info!` listing missing slots so contributors get an early signal without a hard failure.

### `crates/scorpio-core/src/analysis_packs/selection.rs`

- Keep `RuntimePolicy.prompt_bundle` as the transport surface for prompt slots.
- `resolve_runtime_policy_for_manifest(...)` stays shape-only; it must not check active-slot completeness.
- Do not add a large prompt service abstraction.

### `crates/scorpio-core/src/workflow/topology.rs` (new module)

- `RunRoleTopology`, `RoutingFlags`, `Role` enum (with role-to-slot mapping owned in the enum).
- `build_run_topology(...)` — sole constructor.
- `required_prompt_slots(&topology) -> Vec<PromptSlot>` — pure derivation.
- `RoutingFlags::from(&RunRoleTopology)` — pure derivation.
- No second role-to-slot table allowed anywhere. Tests assert grep-level absence of duplicate mappings.

### `crates/scorpio-core/src/workflow/pipeline/runtime.rs`

- Remove `run_analysis_cycle`'s pre-write of `initial_state.analysis_runtime_policy`. `PreflightTask` is the sole writer.
- `build_graph` no longer falls back silently to `PackId::Baseline` when `config.analysis_pack` fails to parse; an unparseable pack id is a typed error returned to the caller. The Baseline-fallback was a pre-centralization safety net that the new "preflight is the failure boundary" contract supersedes.
- Move conditional-edge wiring to read `RoutingFlags` from context (written by preflight) instead of `KEY_MAX_DEBATE_ROUNDS` / `KEY_MAX_RISK_ROUNDS` directly.

### `crates/scorpio-core/src/workflow/tasks/preflight.rs`

- Compute `RunRoleTopology` via `build_run_topology(...)`.
- Compute required slots via `required_prompt_slots(&topology)`.
- Call `validate_active_pack_completeness(...)`. On any missing slot, return `TradingError::Config { pack_id, missing_slots }`.
- Map `TradingError::Config` to `graph_flow::GraphError::TaskExecutionFailed { task: "preflight", source }` at the `Task::run` boundary. The typed error's `Display` representation is preserved inside the `source` string so logs/diagnostics retain the structured information.
- On success, write `RuntimePolicy` and `RoutingFlags::from(&topology)` into context.
- `RunRoleTopology` is dropped at end of `Task::run`; not persisted.

### Activation Paths Audit

Every existing path from a `PackId` or pack-id string to a runnable graph must reach `PreflightTask::run`. The activation-paths audit is a hard PR-merge gate for the migration step that flips enforcement on (PR 4 below). Known paths the audit must cover (non-exhaustive starting list — implementer extends by grep):

- `AnalysisRuntime::run` → `TradingPipeline::run` → graph traversal starting at preflight ✓
- `analyze` CLI subcommand → `AnalysisRuntime::run` ✓
- `registry::resolve_pack(...)` direct callers — must not skip preflight when the resolved policy is used to build a runnable graph
- `PreflightTask::with_pack(...)` callers in tests
- backtest scaffolding (currently empty `crates/scorpio-core/src/backtest/`) — when populated, must use `AnalysisRuntime::run`
- Any test that constructs a pipeline and calls `Task::run` on a non-preflight task — must use the shared `with_baseline_runtime_policy(state)` fixture

### Agent prompt builders

Update prompt-builder signatures so `RuntimePolicy` is a **required parameter**. This is a structural change that makes "missing policy" a compile error, removes the runtime branch entirely, and lets tests' hydration requirements be enforced by the type system rather than convention:

- `trader::prompt::build_prompt_context(state: &TradingState, policy: &RuntimePolicy) -> Result<PromptContext, TradingError>`
- `fund_manager::prompt::build_prompt_context(state: &TradingState, policy: &RuntimePolicy) -> Result<(String, String), TradingError>`
- `researcher::common::DebaterCore::new(state, policy: &RuntimePolicy, ...) -> Result<Self, TradingError>` (already returns `Result`; gain `&RuntimePolicy` parameter)
- `risk::common::RiskAgentCore::new(state, policy: &RuntimePolicy, ...) -> Result<Self, TradingError>` (likewise)
- `analyst::equity::*::system_prompt(policy: &RuntimePolicy, ...) -> Result<String, TradingError>`

Caller behavior:

- Caller tasks (`TraderTask::run`, `FundManagerTask::run`, etc.) read `RuntimePolicy` from context once and pass it down.
- Callers convert any `TradingError::Config` from prompt builders to `graph_flow::GraphError::TaskExecutionFailed { task, source }`, preserving the typed error in the `source` string.
- Callers must never read `state.analysis_runtime_policy.as_ref()` and unwrap inside the prompt builder; the parameter is the source.

Builder behavior:

- Read the assigned slot from `policy.prompt_bundle.<role>`.
- If the slot is blank, return `TradingError::Config { pack_id, slot_name }` — defense-in-depth for the rare case where a pack passed preflight against a different topology than the prompt builder expects.
- Apply runtime substitution and sanitization mechanics only.
- Never panic; never synthesize defaults; never branch to a local fallback constant.

#### Code-owned absence prose (must move to pack assets)

The following code-owned strings encode prompt prose for stage-bypass / missing-input scenarios. They cannot stay in agent code under the structural "rendered == asset modulo placeholders" rule. They move to pack assets in migration step 3:

| Constant / string                             | File                                | Disposition                                                                                           |
|-----------------------------------------------|-------------------------------------|-------------------------------------------------------------------------------------------------------|
| `MISSING_CONSENSUS_NOTE`                      | `agents/trader/prompt.rs`           | Move to `trader` prompt asset; render via a pack-defined `{consensus_summary_or_absence_note}` branch |
| `data_quality_note` (both branches)           | `agents/trader/prompt.rs`           | Move to `trader` prompt asset                                                                         |
| Risk-report `"see user context"` placeholders | `agents/fund_manager/prompt.rs`     | Move to `fund_manager` prompt asset                                                                   |
| `validate_dual_risk_rationale` rationale copy | `agents/fund_manager/validation.rs` | Stays in code (this is a validator, not a prompt) but updated for the new `StageDisabled` variant     |

#### `DualRiskStatus::StageDisabled`

Add a new variant to `DualRiskStatus` for "risk stage was deliberately disabled at zero rounds." This is distinct from `Unknown`, which means "reports raced or are absent due to a bug." Without this distinction, every zero-risk run forces the FM model into the `"indeterminate because"` rationale prefix designed to apologize for degraded missing-data state — semantically wrong for an intentional configuration choice.

- New constructor `DualRiskStatus::from_reports_and_topology(reports, topology) -> DualRiskStatus`. Consults `topology.risk_enabled` first and returns `StageDisabled` when false; otherwise falls back to today's report-presence logic.
- `validate_dual_risk_rationale` accepts `"Dual-risk escalation: stage-disabled because max_risk_rounds = 0 ..."` for `StageDisabled`, and continues to require `"indeterminate because ..."` for `Unknown`.
- The pack's `fund_manager` prompt asset includes the corresponding rationale-prefix instructions for the new variant.

#### Test fixture (replaces legacy `*_SYSTEM_PROMPT` oracles)

Add `crates/scorpio-core/src/testing/runtime_policy.rs` (gated `#[cfg(test)]`) exporting:

- `with_baseline_runtime_policy(state: &mut TradingState)` — hydrates the baseline pack's `RuntimePolicy` for tests that don't go through preflight.
- `baseline_pack_prompt_for_role(role: Role) -> &'static str` — reads the baseline pack's prompt asset for a role. Replaces `*_SYSTEM_PROMPT` constants as the test oracle.

Existing tests that asserted substrings of `TRADER_SYSTEM_PROMPT`, `FUND_MANAGER_SYSTEM_PROMPT`, `FUNDAMENTAL_SYSTEM_PROMPT`, `SENTIMENT_SYSTEM_PROMPT`, etc. migrate to assert against `baseline_pack_prompt_for_role(...)`. The migration is part of the atomic step 4 PR so no tests live with broken oracles.

### Agent-local prompt constants

- Remove agent-local runtime fallback constants once equivalent pack-owned assets are wired (migration step 5).
- Tests use `baseline_pack_prompt_for_role(...)` instead of preserving runtime constants.

## Data Flow

The prompt path after this change:

1. Pack manifest builds `prompt_bundle` from its prompt assets.
2. `resolve_runtime_policy_for_manifest` copies the bundle into `RuntimePolicy` (shape-only).
3. `PreflightTask` calls `build_run_topology(...)` for the run.
4. `PreflightTask` calls `validate_active_pack_completeness(...)` against `required_prompt_slots(&topology)`. On failure, returns `TradingError::Config` with all missing slot names; no synthesis, no partial run.
5. `PreflightTask` writes `RuntimePolicy` and `RoutingFlags::from(&topology)` to context. Topology itself is dropped at end of `Task::run`.
6. Routing edges read `RoutingFlags` to decide bypass behavior for zero-round stages.
7. Agent tasks read `RuntimePolicy` from context and pass it to prompt builders as a required parameter.
8. Prompt builders apply runtime substitution and sanitization mechanics. They return `TradingError::Config` on blank slots; "missing policy" is structurally impossible.
9. The final rendered system prompt is passed to the model.

## Error Handling

- **Missing required slot on the active pack:** hard preflight failure. Error names the active pack id and lists *all* missing slots, not stopping at the first.
- **Inactive stub packs:** do not fail merely by existing in the registry with partial bundles. Registration emits a non-blocking `tracing::info!` listing missing slots so contributors get an early signal.
- **Missing `analysis_runtime_policy` at a prompt-builder entrypoint:** structurally impossible — the `&RuntimePolicy` parameter is required by the type system.
- **Present-but-blank selected prompt slot at a prompt-builder entrypoint:** typed `TradingError::Config { pack_id, slot_name }`. Caller tasks map this to `graph_flow::GraphError::TaskExecutionFailed` with the typed error preserved in the message.
- **Unparseable pack id:** typed error from `build_graph`, not a silent fallback to Baseline.

Preflight is the **primary** failure boundary for orchestrated runs. Prompt-builder blank-slot detection is **defense-in-depth** for any non-preflight caller (tests with manual hydration, alternate entrypoints). With "missing policy" structurally removed via the required parameter, the runtime defense-in-depth case reduces to exactly one scenario: present-but-blank slots, which only happens if a pack passed preflight against a different topology than the prompt builder expects (a real bug worth surfacing loudly rather than masking with an empty render).

## Testing

Add or update coverage for the following:

### Preflight + active-pack validation

- preflight fails closed when the active pack is missing a required prompt slot
- preflight missing-slot error includes the active pack id and all missing slot names
- preflight passes when the active baseline pack has a complete bundle
- preflight required-slot derivation matches `required_prompt_slots(&topology)` for both normal runs and zero-round runs (`max_debate_rounds = 0`, `max_risk_rounds = 0`)
- registration of a stub pack emits a non-blocking `info!` listing missing slots without failing the process
- `validate_active_pack_completeness` is **not** called from `AnalysisPackManifest::validate()` or from `resolve_runtime_policy_for_manifest`
- `validate_active_pack_completeness` **is** called from `PreflightTask::run` and from registry registration

### Topology

- `build_run_topology` is the only constructor for `RunRoleTopology`; grep-level test asserts no other code path constructs one
- `required_prompt_slots(&topology)` and `RoutingFlags::from(&topology)` are pure functions over the same shared `Role` enum; no second role-to-slot table exists in the codebase
- analyst-required prompt-slot derivation matches the same `required_inputs`-plus-registry-filtering path used by analyst task spawning
- `topology.debate_enabled` and `topology.risk_enabled` exactly equal `max_debate_rounds > 0` and `max_risk_rounds > 0` for the current run

### Routing / zero-round bypass

- zero-round debate runs bypass `debate_moderator` entirely (no `Task::run` call recorded for it)
- zero-round risk runs bypass `risk_moderator` entirely
- moderator prompt slots are not required in runs where the corresponding zero-round bypass is active
- zero-round debate runs leave `consensus_summary` absent rather than synthesizing a placeholder
- zero-round risk runs leave risk reports absent and `risk_discussion_history` empty rather than synthesizing placeholders
- Trader prompt builder renders correctly when `consensus_summary` is `None`, using the pack-owned absence-note placeholder (not the deleted code-owned constant)
- Fund Manager `DualRiskStatus::StageDisabled` is produced when `topology.risk_enabled == false`, and is distinct from `Unknown`
- Fund Manager rationale validation accepts the `stage-disabled because ...` prefix for `StageDisabled` and the `indeterminate because ...` prefix for `Unknown`
- end-to-end zero-debate AND zero-risk run completes successfully against a stubbed model and produces a valid `TradeProposal` and `FundManagerDecision`

### Agent prompt builders

- prompt builders take `&RuntimePolicy` as a required parameter (compile-time check; a test that omits the argument must fail to compile under `trybuild` or equivalent)
- prompt builders return `TradingError::Config { pack_id, slot_name }` when the selected slot is blank
- caller tasks map `TradingError::Config` to `graph_flow::GraphError::TaskExecutionFailed` with the typed error preserved in the `source` string
- placeholder rendering such as `{ticker}`, `{current_date}`, and `{analysis_emphasis}` remains intact after centralization
- **rendered baseline prompts equal the extracted pack assets exactly, modulo declared placeholders, for every role.** This is the structural enforcement of the "mechanical only" rule. Every agent role has a roundtrip test: render with known fixture inputs, strip declared placeholder substitutions, assert byte-equality with the pack asset file. A new agent that quietly appends a code-owned sentence will fail this test.

### Migration regression gate (PR 4 merge gate)

- a pre-migration snapshot of every role's rendered system prompt across (a) all-inputs-present, (b) zero-debate, (c) zero-risk, (d) missing-analyst-data scenarios is captured before migration step 4 and asserted byte-equal (modulo declared substitutions) against the post-migration pack-driven path. **This gate must pass before PR 4 lands.** Documented in `docs/solutions/` after migration completes.

### Stub pack

- inactive stub pack still resolves/registers with a partial or empty bundle while remaining non-active
- graph construction / manifest resolution still succeeds for incomplete inactive stub packs because completeness is enforced only in preflight
- registering a stub pack with missing slots emits a non-blocking `info!` listing them
- attempting to set a stub pack as active in config and run `analyze` produces the same hard preflight failure as any other incomplete active pack

### Activation paths

- enumerated paths from `PackId`/string to a runnable graph all traverse `PreflightTask::run` (audit checked into the migration PR)

### Schema version

- `THESIS_MEMORY_SCHEMA_VERSION` bump from 2 to 3 retires pre-migration snapshots from thesis lookup
- thesis-lookup test confirms a snapshot row at version 2 is skipped under version 3

Regression intent:

- prompt prose remains centralized in pack assets
- active-pack validation stays early
- agents remain simple consumers of validated prompt slots
- the "mechanical only" rule is structurally enforced by the rendered=asset test, not by convention

## Migration Notes

This change should be implemented in a way that avoids a long half-migrated state. Each numbered step is a separate PR; **only PR 4 lands its sub-steps atomically**.

1. **PR 1 — topology + helpers (no enforcement, no behavior change).** Add `crates/scorpio-core/src/workflow/topology.rs` (`RunRoleTopology`, `Role` enum, `build_run_topology`, `required_prompt_slots`, `RoutingFlags`). Add `validate_active_pack_completeness` helper, wired into registry registration as a non-blocking `info!` for any pack with missing slots. Add `crates/scorpio-core/src/testing/runtime_policy.rs` (`with_baseline_runtime_policy`, `baseline_pack_prompt_for_role`). Audit and document activation paths from `PackId`/string to graph entry as a checklist in the PR description.

2. **PR 2 — baseline pack completeness.** Ensure baseline pack has complete prompt assets for all `required_prompt_slots(&baseline_topology)` roles. The registration-time `info!` reports zero missing slots for baseline after this PR.

3. **PR 3 — code-owned absence prose migration.** Move `MISSING_CONSENSUS_NOTE`, Trader `data_quality_note` strings, FM risk-report placeholder copy from agent code into pack assets (with new placeholder branches in the assets). Add `DualRiskStatus::StageDisabled` variant and update `validate_dual_risk_rationale`. Capture the pre-migration regression-gate snapshots (rendered prompts for each role across all-inputs-present / zero-debate / zero-risk / missing-analyst-data scenarios). The constants still exist in agent code at this point — they remain the runtime fallback — but the pack assets become the byte-equivalence target for the gate.

4. **PR 4 (atomic) — fallback removal + preflight enforcement + topology-driven routing + schema bump.**
   - 4a. Change prompt-builder signatures to require `&RuntimePolicy`. Migrate all caller tasks. Migrate all tests to use `with_baseline_runtime_policy` and `baseline_pack_prompt_for_role`.
   - 4b. Wire `validate_active_pack_completeness` into `PreflightTask`. Remove `run_analysis_cycle`'s pre-write of `analysis_runtime_policy`. Move conditional edges to read `RoutingFlags` from context. Bump `THESIS_MEMORY_SCHEMA_VERSION` from 2 to 3.
   - **The pre-migration regression gate from PR 3 must pass before merge.** Each role's rendered prompts must match between the PR 3 byte-equivalence snapshot and the post-PR 4 pack-driven path, modulo declared substitutions.

5. **PR 5 — cleanup.** Remove now-dead `*_SYSTEM_PROMPT` constants. Update CLAUDE.md / docs / `docs/solutions/` entry summarizing the migration.

This sequencing keeps the runtime coherent at each step and minimizes the time spent with dual ownership. PRs 1–3 add infrastructure without changing behavior; PR 4 flips behavior atomically with a verification gate; PR 5 cleans up dead code.

## Accepted Tradeoff

The design deliberately allows inactive stub packs to remain incomplete. Prompt completeness is not a universal manifest invariant; it is an invariant only for the active pack at execution time.

This is the right tradeoff for the current repo because crypto scaffolding is intentionally registered before it is runnable. Enforcing universal completeness today would either force low-value placeholder prompt assets or block the existing stub-pack workflow.

The activation-day cliff is mitigated by the registration-time `info!` listing missing slots for any incomplete pack, so contributors get a progressive signal during development rather than a wall of errors on first activation.
