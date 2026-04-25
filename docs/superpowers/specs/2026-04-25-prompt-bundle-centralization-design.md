# Prompt Bundle Centralization Design

**Date:** 2026-04-25
**Status:** Approved

## Goal

Remove prompt ownership duplication by making `AnalysisPackManifest.prompt_bundle` the only runtime prompt source for active packs. Agents should load the prompt for their role from the selected pack, and an active pack with a missing required prompt slot should fail in preflight before any agent runs.

## Problem

The current runtime has two prompt ownership paths:

- pack-owned prompt assets under `analysis_packs/.../prompts/*.md`, carried through `prompt_bundle`
- agent-local fallback constants such as `FUNDAMENTAL_SYSTEM_PROMPT`, `TRADER_SYSTEM_PROMPT`, and `FUND_MANAGER_SYSTEM_PROMPT`

At runtime the code prefers `state.analysis_runtime_policy.prompt_bundle.<slot>` and falls back to the agent-local constant when the slot is empty. That keeps stub packs working, but it leaves prompt ownership duplicated and weakens the architecture:

- the selected pack is not the single source of truth for prompts
- agents still encode policy about missing prompt slots
- prompt correctness is deferred to individual render sites instead of being validated once up front
- new packs can appear valid while depending on unrelated fallback text owned by agent modules

The desired architecture is simpler: the active pack owns the prompts, preflight validates that ownership, and agents only consume the validated slot assigned to their role.

## Decisions

| Decision                 | Choice                                                                          | Rationale                                                                      |
|--------------------------|---------------------------------------------------------------------------------|--------------------------------------------------------------------------------|
| Prompt source of truth   | `AnalysisPackManifest.prompt_bundle` only                                       | Removes duplicated runtime ownership                                           |
| Missing-slot enforcement | Fail only for the active pack                                                   | Stub/inactive packs can remain incomplete during phased development            |
| Failure boundary         | Preflight                                                                       | Fail before fan-out and before any model call                                  |
| Validation scope         | Keep manifest/runtime-policy resolution shape-only                              | Prevent inactive stub packs from failing merely by resolving                   |
| Agent behavior           | No runtime fallback logic                                                       | Keeps agents focused on rendering, not policy                                  |
| Prompt rendering         | Keep placeholder substitution in agent prompt builders                          | Prompt text stays pack-owned; runtime values stay agent-owned                  |
| Missing runtime policy   | Treated as orchestration corruption outside preflight-seeded runs               | Prompt builders should not silently invent defaults once centralization lands  |
| Missing selected slot    | Typed config/orchestration failure even if policy exists                        | Keeps alternate entrypoints and tests consistent with preflight guarantees     |
| Role-to-slot mapping     | One shared canonical prompt-slot identifier surface                             | Avoids duplicating a second role-to-slot table in preflight                    |
| Topology owner           | One typed per-run `RunRoleTopology` built by a single topology-builder function | Preflight validation and graph routing consume the exact same enablement data  |
| Zero-round stage outputs | Leave skipped-stage artifacts absent                                            | Trader/Fund Manager already have absence-handling paths; avoids fake summaries |
| Validation surface       | Small shared helper, not a large prompt service abstraction                     | Right-sized for the current codebase                                           |
| Code-owned prompt prose  | Default target is zero append-only prose                                        | Prevents prompt policy from creeping back into code                            |
| Stub pack support        | Incomplete bundles allowed while pack is inactive                               | Preserves current crypto-pack scaffolding workflow                             |

## Architecture

The architecture becomes:

1. A pack manifest declares its `prompt_bundle`.
2. `resolve_runtime_policy_for_manifest(...)` copies that bundle into `RuntimePolicy`.
3. Preflight calls one topology-builder function that produces a typed per-run `RunRoleTopology` surface.
4. Preflight stores `RunRoleTopology` and any topology-derived routing keys in context/state for later consumers.
5. `RunRoleTopology` drives both routing and prompt-slot requirements for the current run, including bypassing debate/risk moderators entirely when zero-round runs disable those stages.
6. Preflight validates the prompt slots required by that `RunRoleTopology`.
7. Preflight writes the validated `RuntimePolicy` into `TradingState.analysis_runtime_policy` and context.
8. Agent prompt builders load only their assigned slot from the validated policy.
9. Agents render placeholders such as `{ticker}`, `{current_date}`, and `{analysis_emphasis}` and apply only substitution/sanitization mechanics, not append-only prompt prose.

After this change, prompt selection policy exists in one place: the active pack resolution/preflight boundary. Agents do not decide whether a missing prompt is acceptable.

`AnalysisPackManifest::validate()` and `resolve_runtime_policy_for_manifest(...)` remain shape-only. They must not enforce active-run prompt completeness, because that would break the accepted requirement that incomplete stub packs may still resolve while inactive.

## Required Prompt Slots

The active pack must provide every prompt slot needed by the currently runnable roles.

The source of truth for "required prompt roles" must be a single typed per-run `RunRoleTopology` surface built by one topology-builder function. That function is the sole owner of whatever inputs determine runnable roles for the current run. If routing later depends on additional inputs such as provider capabilities, resolved instrument details, or any future topology gate, that function absorbs them and remains the only enablement source of truth.

`RunRoleTopology` must expose canonical prompt-slot identifiers directly, or use one shared enum that owns the role-to-slot mapping in one place. Preflight validation and graph routing must both consume the exact same topology-builder function / `RunRoleTopology` result; a second preflight-only role-to-slot table is not allowed.

As of today's workflow topology, `RunRoleTopology` should yield:

- always required topology-fixed roles:
  - `trader`
  - `fund_manager`
- analyst prompt slots for the analyst roles actually spawned for the active pack, derived by the topology-builder from the same analyst-selection path routing uses
- additionally required when `RunRoleTopology` marks debate enabled for the current run:
  - `bullish_researcher`
  - `bearish_researcher`
  - `debate_moderator`
- additionally required when `RunRoleTopology` marks risk enabled for the current run:
  - `aggressive_risk`
  - `conservative_risk`
  - `neutral_risk`
  - `risk_moderator`

This keeps zero-round workflows valid without forcing unused prompt slots to be present on the active pack, because zero-round runs should bypass moderator tasks entirely rather than routing through them once. It also prevents future non-equity packs from failing preflight on analyst prompts they never run.

When a stage is bypassed at zero rounds, its downstream artifacts remain absent rather than being synthesized deterministically:

- zero-debate runs leave `consensus_summary` absent and do not create moderator output for the skipped stage
- zero-risk runs leave the three risk reports absent and keep `risk_discussion_history` empty for the skipped stage

Downstream consumers must continue to handle true absence through their existing missing-input behavior rather than relying on placeholder summaries.

For the baseline equity pack, all of these slots should be populated from extracted prompt assets.

For incomplete stub packs such as the current crypto placeholder, empty or partial bundles remain acceptable only while the pack is not active. The active-pack validation boundary enforces this without blocking scaffold manifests from existing in the registry.

## Component Changes

### `crates/scorpio-core/src/analysis_packs/manifest/schema.rs`

- Keep `prompt_bundle` on `AnalysisPackManifest` as the canonical prompt container.
- Add or wire a validation helper for active-pack prompt completeness, but do not enforce global non-empty validation for every registered pack.
- `AnalysisPackManifest::validate()` should continue to validate manifest shape broadly without making inactive stub packs impossible to register.

### `crates/scorpio-core/src/analysis_packs/selection.rs`

- Keep `RuntimePolicy.prompt_bundle` as the transport surface for prompt slots.
- Add a small helper if needed for validating or reading required prompt slots from a resolved runtime policy.
- Do not add a large prompt service abstraction.
- Do not move active-slot completeness checks into `resolve_runtime_policy_for_manifest(...)`; that function must stay usable for inactive stub packs.

### `crates/scorpio-core/src/workflow/pipeline/runtime.rs` and shared workflow topology surface

- Introduce one typed per-run `RunRoleTopology` surface that computes enabled prompt-consuming roles and canonical prompt-slot identifiers for the run.
- Build it with one topology-builder function that is the sole source of truth for role enablement.
- The topology-builder owns the complete input set for runnable-role decisions; callers must not mirror or partially reconstruct that logic.
- Preflight computes and persists `RunRoleTopology`; graph routing and preflight validation must consume that exact stored result or topology-derived keys, not rebuild enablement independently later in the run.
- Zero-round behavior changes as part of this design: when `RunRoleTopology` marks debate or risk disabled, routing must bypass the corresponding moderator task entirely instead of visiting it once.
- The bypass contract leaves skipped-stage artifacts absent rather than writing synthetic summaries or placeholder reports.
- If legacy scalar context keys remain, they must be derived from `RunRoleTopology`, not maintained independently.

### `crates/scorpio-core/src/workflow/tasks/preflight.rs`

- Preflight becomes the enforcement boundary for active-pack prompt completeness.
- Preflight also becomes the point where `RunRoleTopology` is built and persisted for the run.
- After resolving the active `RuntimePolicy`, validate that every role required by the current run's `RunRoleTopology` has a non-empty slot.
- Preflight must consume the same topology-builder function / `RunRoleTopology` surface used for routing, not recompute enablement from config defaults or a preflight-local mirror.
- Because zero-round runs bypass moderators entirely in this design, moderator prompt slots are not required when `RunRoleTopology` marks the corresponding stage as disabled.
- On any missing required slot, return a hard task failure before analyst fan-out begins.
- The error should include the active pack id plus the full list of missing slot names in a single actionable message.
- Preserve the current state/context write path after validation passes.

### Agent prompt builders

Update the active prompt-builder surfaces so they consume the selected pack slot directly rather than branching on fallback constants.

Affected areas include:

- `crates/scorpio-core/src/agents/analyst/equity/*.rs`
- `crates/scorpio-core/src/agents/researcher/common.rs`
- `crates/scorpio-core/src/agents/trader/prompt.rs`
- `crates/scorpio-core/src/agents/risk/common.rs`
- `crates/scorpio-core/src/agents/fund_manager/prompt.rs`

Expected runtime behavior:

- read the already-validated slot from `state.analysis_runtime_policy`
- render placeholders and sanitize runtime values
- never branch to a local fallback prompt constant

If `state.analysis_runtime_policy` is absent at a runtime prompt-builder entrypoint, return a typed `TradingError::Config` describing orchestration corruption / missing runtime policy. If the policy exists but the selected slot is blank or missing, return the same typed `TradingError::Config` family rather than rendering an empty prompt, panicking, or inventing a default. Prompt builders must never panic and never synthesize defaults. Tests and alternate entrypoints should hydrate a runtime policy explicitly when they exercise these paths.

Prompt ownership after centralization:

- pack-owned:
  - role instructions
  - role tone/voice
  - schema/task wording
  - role-specific analytical guidance
- code-owned only:
  - runtime placeholder substitution (`{ticker}`, `{current_date}`, `{analysis_emphasis}`, etc.)
  - prompt sanitization and serialization of runtime context

Code-owned text must stay mechanical. It must not reintroduce substantive prompt policy that belongs in pack assets. The default target for this change is zero append-only prose in code. Existing role-specific guardrail sentences that materially affect prompt semantics should move into pack assets as part of centralization rather than remaining in agent modules.

### Agent-local prompt constants

- Remove agent-local runtime fallback constants once equivalent pack-owned assets are wired.
- If any prompt text is still useful as a regression baseline during migration, tests may compare against extracted assets directly rather than preserving runtime constants indefinitely.

The target state is that agent modules no longer own canonical prompt text.

## Data Flow

The prompt path after this change:

1. Pack manifest builds `prompt_bundle` from its prompt assets.
2. Runtime policy copies the bundle.
3. Preflight computes and stores `RunRoleTopology` for the run.
4. `RunRoleTopology` decides which prompt-consuming roles will actually run, and bypasses zero-round moderators entirely.
5. Preflight validates the active policy's required slots for those roles only.
6. Validated policy is stored on `TradingState`.
7. Agent prompt builders read the slot for their role.
8. Builders apply only runtime substitution and sanitization mechanics.
9. The final rendered system prompt is passed to the model.

This keeps prompt content pack-driven while leaving runtime values and role-specific formatting in the agent layer.

## Error Handling

- Missing required slot on the active pack is a hard preflight failure.
- The error should clearly name the missing slot so the broken pack can be fixed quickly.
- The error should include all missing required slots, not stop at the first one.
- Inactive stub packs do not fail merely by existing in the registry with partial bundles.
- Once preflight succeeds, prompt rendering should not encounter "missing prompt" as a runtime condition.
- Missing `analysis_runtime_policy` at a prompt-builder entrypoint is a typed `TradingError::Config` for orchestration corruption / missing runtime policy, not a panic and not a fallback case.
- Present-but-blank selected prompt slots at a prompt-builder entrypoint are also typed `TradingError::Config` failures, not empty renders and not implicit defaults.

This narrows prompt errors to one early failure boundary instead of scattering them across agent constructors and prompt renderers.

## Testing

Add or update coverage for the following:

- preflight fails closed when the active pack is missing a required prompt slot
- preflight missing-slot error includes the active pack id and all missing slot names
- preflight passes when the active baseline pack has a complete bundle
- preflight required-slot derivation matches the shared workflow/topology source of truth for both normal runs and zero-round runs (`max_debate_rounds = 0`, `max_risk_rounds = 0`)
- one topology-builder function / `RunRoleTopology` surface is consumed by both graph routing and preflight validation
- shared role/topology surface yields canonical prompt-slot identifiers without a duplicated preflight-only mapping table
- `RunRoleTopology` is the single enablement surface consumed by both graph routing and preflight validation
- preflight computes and persists `RunRoleTopology`, and later routing reads that stored result or topology-derived keys rather than recomputing enablement
- analyst-required prompt-slot derivation matches the same `required_inputs` plus registry-filtering path used for analyst task spawning
- debate/risk-required prompt-slot derivation reads the same current-run round-count source used by graph routing
- zero-round debate runs bypass `debate_moderator` entirely
- zero-round risk runs bypass `risk_moderator` entirely
- moderator prompt slots are not required in runs where the corresponding zero-round bypass is active
- zero-round debate runs leave `consensus_summary` absent rather than synthesizing a placeholder
- zero-round risk runs leave risk reports absent and `risk_discussion_history` empty rather than synthesizing placeholders
- Trader and Fund Manager tests cover the true-absence contract for zero-round bypass runs
- active agent prompt builders consume the bundle slot directly with no fallback path remaining
- prompt-builder/runtime entrypoints return typed `TradingError::Config` errors when `analysis_runtime_policy` is absent
- prompt-builder/runtime entrypoints return typed `TradingError::Config` errors when the selected slot is blank
- baseline pack prompt assets populate all runtime-required slots
- placeholder rendering such as `{ticker}`, `{current_date}`, and `{analysis_emphasis}` remains intact after centralization
- inactive stub pack can still resolve/register with partial or empty bundle while remaining non-active
- graph construction / manifest resolution still succeeds for incomplete inactive stub packs because completeness is enforced only in preflight
- rendered baseline prompts match the extracted assets exactly aside from runtime substitution and sanitization mechanics

Regression intent:

- prompt ownership remains centralized in pack assets
- active-pack validation stays early
- agents remain simple consumers of validated prompt slots

## Migration Notes

This change should be implemented in a way that avoids a long half-migrated state.

Recommended order:

1. Add `RunRoleTopology` plus the active-pack prompt-slot validation helper/tests without enabling hard preflight failure yet.
2. Ensure baseline pack has complete prompt assets for all runnable roles.
3. Move any remaining role-specific append-only prompt prose out of agent modules into pack assets.
4. Atomically remove per-agent fallback logic and enable hard preflight enforcement in the same change. Do not merge a runtime state where only one of those two sides has landed.
5. Remove now-dead runtime prompt constants and stale documentation.

This keeps the runtime coherent at each step and minimizes the time spent with dual ownership.

## Accepted Tradeoff

The design deliberately allows inactive stub packs to remain incomplete. That means prompt completeness is not a universal manifest invariant; it is an invariant only for the active pack at execution time.

This is the right tradeoff for the current repo because crypto scaffolding is intentionally registered before it is runnable. Enforcing universal completeness today would either force low-value placeholder prompt assets or block the existing stub-pack workflow.
