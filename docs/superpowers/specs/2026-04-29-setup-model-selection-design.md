# Design — upgrade `rig-core` to `0.36.0`, remove custom Copilot, and add setup model selection via provider listing

**Date:** 2026-04-29
**Author:** brainstorming session with BigtoC
**Status:** Draft — pending written-spec review and implementation plan

## Summary

Upgrade the workspace to `rig-core 0.36.0`, then improve `scorpio setup` step 4 so quick-thinking and deep-thinking model selection can use provider-discovered model lists instead of manual text entry alone. The wizard should prefetch model lists once per setup run, in parallel, for eligible keyed providers, then keep the existing provider-first flow: choose provider first, then choose a listed model or enter one manually.

`rig-core 0.36.0` also removes `FinalCompletionResponse`, which breaks Scorpio's custom ACP-based Copilot provider in `crates/scorpio-core/src/providers/copilot.rs`. Rather than porting custom Copilot code that the user intends to replace immediately, this design removes the current custom Copilot implementation and its related runtime/config wiring. Official `rig`-native Copilot support is explicitly deferred to the next task.

The key product decisions are:

- only providers with user-configured API keys participate in model listing
- `openrouter` is manual-only and is never queried for models
- model discovery is best-effort and never blocks setup
- manual entry is always available as the final option
- all returned model IDs are shown as-is, with no filtering

## Goals

- Upgrade workspace `rig-core` to `0.36.0`.
- Remove Scorpio's custom ACP-based Copilot provider so the `rig-core 0.36.0` upgrade can land cleanly.
- Add provider-backed model discovery for setup step 4.
- Fetch model lists once per setup run, in parallel, for eligible providers.
- Preserve the current quick/deep routing mental model: provider selection first, model selection second.
- Preserve manual model entry for every provider path.
- Use DeepSeek's standard upstream model-listing support after the `rig-core` upgrade.
- Update setup-facing docs to reflect the new model-selection UX and the temporary Copilot removal.

## Non-goals

- Do not add official `rig` Copilot support in this change.
- Do not keep the custom Copilot ACP implementation alive behind compatibility shims.
- Do not patch `graph-flow` in this change; if `rig-core 0.36.0` proves incompatible, that prerequisite patch is handled separately by the user before this work continues.
- Do not add OpenRouter model listing.
- Do not add a refresh action to step 4; discovery happens once per `scorpio setup` run.
- Do not filter, rank, or curate provider model lists; show every returned model ID.
- Do not redesign steps 1, 2, 3, or 5 beyond the targeted changes needed for Copilot removal and step-4 listing.

## Design choices

| Decision | Choice | Rationale |
|---|---|---|
| Dependency baseline | Upgrade `rig-core` to `0.36.0` only | Approved user scope; `graph-flow` is handled separately only if it blocks the upgrade |
| Copilot posture | Remove custom ACP Copilot entirely | `FinalCompletionResponse` is removed upstream, and the user plans to add official `rig` Copilot next |
| Copilot compatibility | No temporary compatibility layer | Avoids migrating the same provider twice and keeps this change focused |
| Model discovery ownership | New `providers/factory/discovery.rs` module in `scorpio-core` | `client.rs` is already very large; discovery belongs in core, not CLI |
| Eligible providers | Only providers with keys present in wizard state | Matches approved user scope |
| Fetch strategy | Prefetch once at step start, in parallel | Approved user scope; stable quick/deep UX |
| Prompt flow | Provider select, then model select/manual entry | Approved user scope; avoids one huge mixed model list |
| OpenRouter behavior | Manual-only with a short note | Approved user scope; avoids an unbounded model list |
| Listing failure | Skip listing for that provider and use manual entry | Approved user scope; listing is opportunistic |
| Manual entry option | Always append `Enter model manually` last | Approved user scope; supports newly released or private models |
| Returned model list | Show every returned model ID | Approved user scope |
| Duplicate returned model IDs | Preserve as returned | Matches the approved rule to show every returned model ID without curation |
| Saved model behavior | If listed, show first; otherwise default to manual with prefilled text | Approved user scope |
| Saved provider fallback | If unsupported or ineligible, ignore it and default to the first eligible provider | Keeps setup usable as the recovery path for stale configs |
| Refresh behavior | None | Approved user scope |

## Architecture

The implementation has two tightly related tracks that should be treated as one design:

1. The workspace upgrades to `rig-core 0.36.0`.
2. The custom Copilot provider and ACP transport are removed from Scorpio's runtime surface.
3. `scorpio-core` gains a narrow, provider-agnostic model-discovery seam under `providers/factory/`.
4. `scorpio-cli` step 4 uses that seam to prefetch provider model lists once, then drives the quick/deep prompts from a cached discovery snapshot.

The resulting runtime provider surface for this change is temporarily:

- `openai`
- `anthropic`
- `gemini`
- `openrouter`
- `deepseek`

`copilot` is intentionally absent until the follow-up task that adds official `rig` Copilot support.

This keeps the crate boundary clean:

- `scorpio-core` owns provider IDs, provider settings, `rig` clients, model discovery, and error sanitization
- `scorpio-cli` owns interactive prompt flow and display text

## Components

### Workspace dependencies

`Cargo.toml`

- Update workspace `rig-core` pin to `0.36.0`.
- Do not patch `graph-flow` as part of this change.
- If `graph-flow` blocks compilation or dependency resolution after the `rig-core` bump, stop this implementation slice and wait for the separate user-owned patch.

`Cargo.lock`

- Refresh the lockfile for the `rig-core 0.36.0` upgrade.

### Temporary Copilot removal

`crates/scorpio-core/src/providers/mod.rs`

- Remove `pub mod copilot;`.
- Remove `ProviderId::Copilot`.
- Update `ProviderId::as_str()`, `missing_key_hint()`, display tests, and any provider-list comments accordingly.

`crates/scorpio-core/src/providers/acp.rs`

- Delete the ACP transport module. It only exists to support the custom Copilot implementation.

`crates/scorpio-core/src/providers/copilot.rs`

- Delete the custom Copilot provider implementation.
- This removes the direct dependency on the upstream `FinalCompletionResponse` type that no longer exists in `rig-core 0.36.0`.

`crates/scorpio-core/src/config.rs`

- Remove `copilot` from accepted provider names and supported-provider error text.
- Remove `ProvidersConfig.copilot` and any related defaults/helpers.
- Keep provider-name validation accurate for the temporary supported set: `openai`, `anthropic`, `gemini`, `openrouter`, `deepseek`.

`crates/scorpio-core/src/settings.rs`

- Keep `PartialConfig` provider/model routing fields as raw optional strings with no provider-name validation.
- This lets `scorpio setup` continue to load stale `copilot` selections from `~/.scorpio-analyst/config.toml` and overwrite them through the normal wizard path.

`crates/scorpio-core/src/rate_limit.rs`

- Remove the Copilot rate-limiter slot and related tests.

`crates/scorpio-core/src/providers/factory/client.rs`

- Remove `ProviderClient::Copilot`.
- Remove Copilot-specific preflight support.
- Remove `SCORPIO_COPILOT_CLI_PATH` validation helpers and their tests.
- Keep this file focused on completion-model construction for the remaining providers.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Remove the Copilot agent variant and imports.
- Keep the provider dispatch enum aligned with the remaining provider set.

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Remove the setup health-check Copilot preflight path.
- Remove Copilot-specific tests and helper branches that only existed because `ProviderId::Copilot` remained in shared enums.

`README.md`

- Remove or rewrite the current custom-Copilot limitation section so public docs reflect the actual post-change state.
- The resulting docs should be factual: Copilot is temporarily unavailable in this change and will be reintroduced later through official `rig` support.

This design intentionally treats Copilot as temporarily unsupported rather than preserving a transitional compatibility layer.

### Model discovery seam in `scorpio-core`

Because `crates/scorpio-core/src/providers/factory/client.rs` is already large, model discovery should live in a dedicated new submodule:

- `crates/scorpio-core/src/providers/factory/discovery.rs`

`crates/scorpio-core/src/providers/factory/mod.rs`

- Re-export a narrow discovery API for CLI consumers.

The discovery module should expose a small provider-agnostic surface, for example:

- a discovery outcome enum with three explicit states:
  - `Listed(Vec<String>)`
  - `ManualOnly { reason }`
  - `Unavailable { reason }`
- a function that accepts the eligible providers plus effective `ProvidersConfig`
- a function that fetches all eligible outcomes in parallel and returns a provider-keyed map

The module's responsibilities are:

- create the smallest provider-specific `rig` client needed to list models
- reuse the same provider settings concepts Scorpio already uses for completion clients:
  - API key
  - optional `base_url`
- convert provider-specific model metadata into ordered `Vec<String>` based on model IDs
- preserve provider/API order exactly, including duplicate IDs if the upstream provider returns them
- normalize an empty successful provider response into `Unavailable { reason }` rather than presenting an empty picker
- sanitize provider-specific discovery errors before exposing them to CLI callers
- short-circuit `openrouter` into `ManualOnly` without making a network request

This change does not require a new cross-provider abstraction shared with completion-model construction. A focused discovery module is sufficient.

### Step 4 wizard UX

`crates/scorpio-cli/src/cli/setup/steps.rs`

Step 4 should keep the current high-level flow:

1. Prompt for quick-thinking provider.
2. Resolve the selected provider's model-selection behavior.
3. Prompt for quick-thinking model.
4. Prompt for deep-thinking provider.
5. Resolve the selected provider's model-selection behavior.
6. Prompt for deep-thinking model.

The changes are:

- keep `eligible = providers_with_keys(partial)` as the provider gate
- prefetch discovery results once, in parallel, for all eligible providers before either tier prompt
- reuse the same prefetched results for both the quick-thinking and deep-thinking prompts

For each tier:

- if the selected provider is `Listed(models)`:
  - show all model IDs returned by the provider
  - if the saved model is present, move it to the first position and preserve provider order for the rest
  - append `Enter model manually` as the last option
- if the saved model is not present in the listed models:
  - default the picker to `Enter model manually`
  - prefill the manual text prompt with the saved model value
- if the selected provider is `ManualOnly { reason }` or `Unavailable { reason }`:
  - print a short note
  - skip the picker and go straight to manual text entry

Manual text entry remains validated exactly as today: the model name must not be empty.

Saved-provider fallback is explicit:

- if the saved provider is still eligible, keep using it as the default provider selection
- if the saved provider is unsupported after the Copilot removal, or is no longer eligible because no key is present, ignore it and default the provider prompt to the first eligible provider
- if the user changes providers, the model from the previously selected provider is not silently carried across
- manual prefill only uses the current tier's saved model when that saved model belongs to the currently selected provider path

### Discovery bootstrap inside setup

Step 4 should treat model discovery as best-effort. The wizard must not become more fragile just because it now offers provider-backed model lists.

That means:

- if per-provider listing fails, only that provider degrades to manual entry
- if the step-4 discovery bootstrap fails before any provider request can run, the wizard should degrade all eligible providers to `Unavailable` and continue with manual entry rather than aborting setup

This keeps the existing `inquire`-driven prompt flow intact and preserves the approved rule that listing is opportunistic, not required.

### Public docs

`README.md`

- Update setup step 4 documentation so it no longer describes only raw text entry.
- Document that listed providers can present a fetched model menu plus `Enter model manually`.
- Document that `openrouter` remains manual-only.
- Remove or replace any statements that imply custom Copilot is still available.
- Limit documentation changes in this slice to `README.md`; no historical design docs or `.env.example` updates are required.

No `.env.example` change is required for Copilot because the current custom path used `SCORPIO_COPILOT_CLI_PATH`, not an API key entry in `.env.example`.

## Runtime flow

1. The user runs `scorpio setup`.
2. Steps 1-3 continue to collect financial-data keys and LLM provider API keys.
3. Step 4 computes the eligible provider set from `PartialConfig` using the same keyed-provider gate it already uses today.
4. Step 4 builds effective provider settings from the current wizard state.
5. `scorpio-core::providers::factory::discovery` fetches discovery outcomes for the eligible providers in parallel.
6. For `openrouter`, discovery returns `ManualOnly` immediately without hitting a models endpoint.
7. For OpenAI, Anthropic, Gemini, and DeepSeek, discovery attempts to list models through `rig-core 0.36.0` and normalizes the results into ordered model-ID lists.
8. The quick-thinking prompt selects a provider, then uses the cached discovery outcome to drive either a model picker or manual text prompt.
9. The deep-thinking prompt repeats the same process against the same cached discovery snapshot.
10. Step 5 runs the normal health check against the selected quick/deep providers and models, without any custom Copilot preflight path.

## Error handling

### Dependency-upgrade posture

- The `rig-core 0.36.0` bump is the first operation in this change.
- The removal of `FinalCompletionResponse` is not handled by adapting the custom Copilot provider; the custom provider is deleted instead.
- If `graph-flow` is incompatible with `rig-core 0.36.0`, stop and wait for the separate user-owned `graph-flow` patch before continuing the rest of this implementation.

### Temporary Copilot removal

- `copilot` is intentionally not a supported provider after this change.
- `scorpio analyze` and any other runtime config load that still resolves `copilot` will fail with the normal unsupported-provider validation error until the user reruns setup or edits config.
- `scorpio setup` remains the intended recovery path for stale Copilot configs because it loads `PartialConfig`, not the validated runtime `Config` shape.
- A Copilot-only existing config is recoverable through setup: step 3 will require the user to add at least one supported keyed provider before step 4 can continue.
- This is an explicit product trade-off in this spec, not an accidental regression.

### Model discovery failures

- Discovery failures are non-fatal at the provider level.
- `Unavailable` reasons should be sanitized before being shown in CLI text.
- Discovery must not leak API keys or raw sensitive response bodies.
- A provider discovery failure must not affect other providers' listings.
- A provider response that succeeds but returns zero models is treated as `Unavailable`, with a user-facing message that the provider returned no models.

The CLI text contract should stay stable enough for tests:

- `Model listing is manual-only for openrouter; enter the model manually.`
- `Could not load models for <provider>; enter the model manually.`
- `No models were returned for <provider>; enter the model manually.`

### Setup step behavior

- Step 4 should never abort solely because model listing was unavailable.
- If discovery succeeds, the user gets the richer picker UX.
- If discovery fails, the user still has a working manual-entry setup path.

## Testing

### Dependency and Copilot-removal coverage

`crates/scorpio-core/src/config.rs`

- Update provider-name tests so `copilot` is no longer accepted.
- Update supported-provider error text assertions.

`crates/scorpio-core/src/providers/mod.rs`

- Remove Copilot-specific enum/display tests.

`crates/scorpio-core/src/rate_limit.rs`

- Remove Copilot limiter tests and ensure the remaining provider set still maps correctly.

`crates/scorpio-core/src/providers/factory/client.rs`

- Remove Copilot client-construction and CLI-path validation tests.
- Keep completion-model construction tests for the remaining providers intact.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Remove Copilot agent-construction coverage.
- Keep the dispatch and build-agent coverage for the remaining providers intact.

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Remove Copilot-specific health-check/preflight tests.

### Model discovery coverage

`crates/scorpio-core/src/providers/factory/discovery.rs`

- Add tests for `openrouter -> ManualOnly`.
- Add tests for successful model-list normalization for listed providers.
- Add tests that provider order from upstream is preserved.
- Add tests that failures normalize into `Unavailable` with sanitized summaries.
- Add tests that the parallel discovery aggregator returns one outcome per eligible provider.

### Step 4 wizard coverage

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Add tests that prefetched discovery results are reused for both tiers.
- Add tests that listed models include `Enter model manually` last.
- Add tests that a saved listed model is promoted to the first position.
- Add tests that a saved unlisted model defaults the flow to manual entry with prefilled text.
- Add tests that `ManualOnly` and `Unavailable` skip the picker and go directly to text entry.
- Keep `providers_with_keys` as the only provider-eligibility filter.

### Verification commands

Implementation is only complete after all required repo checks pass in order:

1. `cargo fmt -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## Implementation boundaries

The resulting implementation plan should stay inside these boundaries:

- start with the `rig-core 0.36.0` upgrade only
- if `graph-flow` blocks that upgrade, stop and wait for the separate patch instead of mixing it into this change
- remove the custom Copilot path completely rather than adapting it to `0.36.0`
- do not add official `rig` Copilot in this change
- keep model discovery in a dedicated factory submodule rather than adding more weight to `client.rs`
- keep the CLI provider-first flow intact
- never query OpenRouter for model listing
- always retain manual model entry
- never filter returned model IDs beyond the approved saved-model promotion rule

## Approved direction

This design reflects the validated brainstorming decisions:

- `rig-core` moves to `0.36.0`.
- The custom Copilot implementation is removed now; official `rig` Copilot is deferred to the next task.
- Setup step 4 keeps provider selection first and model selection second.
- Eligible providers are limited to providers with keys set in the wizard state.
- Model discovery is prefetched once per setup run, in parallel.
- `openrouter` is manual-only.
- Manual entry is always available as the last option.
- If listing fails for a provider, that provider falls back to manual entry without aborting setup.
- If the saved model is still listed, it appears first; otherwise manual entry is prefilled.
- All returned model IDs are shown without filtering.
