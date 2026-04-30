# Design — add native GitHub Copilot and Xiaomi MiMo providers

**Date:** 2026-04-30
**Author:** brainstorming session with BigtoC
**Status:** Draft — pending written-spec review and implementation plan

## Summary

Add GitHub Copilot and Xiaomi MiMo as first-class LLM providers across Scorpio's existing provider seams: config validation, provider settings, rate limiting, runtime client construction, setup routing, model selection, and public docs.

The key architectural decision is to use `rig-core 0.36.0`'s native providers directly rather than reviving Scorpio's removed custom Copilot runtime or routing Xiaomi MiMo through a generic OpenAI-compatible alias. Copilot returns as the canonical `copilot` provider using rig's OAuth-capable client and a Scorpio-owned token cache directory. Xiaomi MiMo is added as the canonical `xiaomimimo` provider using rig's native OpenAI-compatible Xiaomi MiMo client and a normal API-key flow.

The key product decisions are:

- `copilot` is routable in setup even when no LLM API key has been saved
- Copilot auth uses rig's OAuth/device-flow support, not a Scorpio-managed API key
- Copilot setup model selection uses a curated static list plus manual entry
- Xiaomi MiMo is exposed as one provider, not split into OpenAI-compatible and Anthropic-compatible variants
- Xiaomi MiMo setup model selection uses live `list_models()` results when possible and manual entry otherwise

## Goals

- Reintroduce GitHub Copilot as a supported runtime provider after the recent temporary removal.
- Add Xiaomi MiMo as a new first-class runtime provider.
- Accept `copilot` and `xiaomimimo` anywhere Scorpio currently accepts a quick-thinking or deep-thinking provider name.
- Keep both providers inside Scorpio's existing provider architecture rather than special-casing them at call sites.
- Support Copilot without requiring `SCORPIO_*` API-key storage.
- Support Xiaomi MiMo through the same keyed-provider config seams as the existing HTTP providers.
- Surface both providers in `scorpio setup`.
- Keep setup model selection aligned with provider capabilities: Copilot uses a curated static list, while Xiaomi MiMo uses provider-backed listing when no custom base URL is configured.
- Update docs and examples so the supported-provider surface is accurate again.

## Non-goals

- Do not restore Scorpio's old ACP-based Copilot implementation.
- Do not add a second Xiaomi MiMo provider ID for the Anthropic-compatible API.
- Do not add a generic "compatible provider" abstraction layer for providers that already have native rig support.
- Do not add Copilot-specific secret storage inside Scorpio config files.
- Do not add Copilot model discovery from a live endpoint; use a static curated list for setup.
- Do not redesign the broader provider factory beyond the targeted enum/config/setup extensions needed for these providers.

## Design choices

| Decision | Choice | Rationale |
|---|---|---|
| Copilot integration | Native `rig::providers::copilot` client | Reuses upstream auth/runtime behavior and avoids reviving deleted custom runtime code |
| Xiaomi MiMo integration | Native `rig::providers::xiaomimimo::Client` | Matches upstream provider identity and preserves model-listing support |
| Xiaomi MiMo provider shape | One provider: `xiaomimimo` | Approved user scope; avoids exposing API-dialect details in Scorpio config |
| Copilot auth mode | OAuth/device flow by default | Approved user scope; no Scorpio-managed `SCORPIO_COPILOT_API_KEY` |
| Copilot token cache location | `~/.scorpio-analyst/github_copilot/` | Keeps Scorpio-owned auth state under the project config root instead of rig's global default |
| Copilot setup routing | Always selectable | Approved user scope; Copilot must be usable without first saving a key |
| Copilot setup models | Static curated list plus manual entry | Rig Copilot does not expose model listing; user explicitly chose a static list |
| Xiaomi MiMo setup models | Live listing when `base_url` is absent; manual fallback otherwise | Matches approved user scope and current setup behavior for custom base URLs |
| Setup provider groups | Split keyed providers from routable providers | Copilot is routable without a key; current single-list setup shape is no longer sufficient |
| Saved Copilot compatibility | Treat saved `copilot` routes as valid again | The recent stale-Copilot recovery path should disappear once Copilot is supported again |
| Default RPM posture | Conservative defaults for both new providers | Avoid optimistic quota assumptions for newly added providers |

## Architecture

The existing provider layer in `crates/scorpio-core/src/providers/` remains the sole runtime integration seam.

The target architecture is:

1. `ProviderId` gains `Copilot` and `XiaomiMimo`.
2. Runtime config accepts the canonical provider names `copilot` and `xiaomimimo`.
3. `ProvidersConfig` gains `[providers.copilot]` and `[providers.xiaomimimo]` sections using the same `ProviderSettings` shape already used by other providers.
4. `PartialConfig` gains Xiaomi MiMo secret support plus non-secret base URL / RPM fields for both providers.
5. `create_completion_model(...)` resolves `copilot` into a rig Copilot client and `xiaomimimo` into a rig Xiaomi MiMo client.
6. `build_agent(...)` and the retry helpers continue to present Scorpio's provider-agnostic LLM interface.
7. `scorpio setup` stops treating "providers that have keys" and "providers that can be routed" as the same concept.
8. Copilot model selection is setup-only static data; Xiaomi MiMo model selection is provider-backed when listing is available.

This keeps the change inside the same seams that already own OpenAI, Anthropic, Gemini, OpenRouter, and DeepSeek.

## Components

### Provider identity and validation

`crates/scorpio-core/src/providers/mod.rs`

- Add `ProviderId::Copilot` and `ProviderId::XiaomiMimo`.
- Extend `ProviderId::as_str()` with `"copilot"` and `"xiaomimimo"`.
- Keep `ProviderId::missing_key_hint()` meaningful for key-based providers; Copilot must bypass generic missing-key error construction entirely.
- Update display and provider-ID tests to cover the expanded provider set.

`crates/scorpio-core/src/config.rs`

- Accept `copilot` and `xiaomimimo` in provider-name deserialization.
- Update all supported-provider error messages to include the two new provider names.
- Remove the stale-Copilot compatibility marker and the friendly "Copilot has been removed" recovery wrapper around `load_from_user_path(...)`.
- Extend `ProvidersConfig` with `copilot: ProviderSettings` and `xiaomimimo: ProviderSettings`.
- Add provider defaults for both new sections.
- Extend helper lookups like `settings_for(...)`, `base_url_for(...)`, `rpm_for(...)`, and `api_key_for(...)`.
- Add Xiaomi MiMo env-secret loading via `SCORPIO_XIAOMIMIMO_API_KEY`.
- Do not add a Scorpio-specific Copilot API-key env contract.
- Adjust "no LLM provider API key found" warnings so Copilot-only routing does not produce a misleading warning.

The important config rule is:

- Xiaomi MiMo behaves like the existing keyed providers.
- Copilot behaves like a configured provider without requiring Scorpio-managed secret storage.

### Persisted setup boundary

`crates/scorpio-core/src/settings.rs`

- Add `xiaomimimo_api_key: Option<String>` to `PartialConfig` and the flat user-config file shape.
- Add `copilot_base_url`, `copilot_rpm`, `xiaomimimo_base_url`, and `xiaomimimo_rpm` as non-secret persisted fields.
- Do not add `copilot_api_key`.
- Extend the redacted `Debug` implementation and the round-trip/load/save tests accordingly.
- Add or extract a helper for the Scorpio state/config directory so Copilot token storage can be derived without duplicating `HOME` resolution logic.

### Provider construction

`crates/scorpio-core/src/providers/factory/client.rs`

- Import `rig::providers::{copilot, xiaomimimo}`.
- Extend `ProviderClient` with `Copilot(copilot::Client)` and `XiaomiMimo(xiaomimimo::Client)`.
- Extend `validate_provider_id(...)` so `copilot` and `xiaomimimo` resolve cleanly.
- Add a Copilot client-construction branch using `copilot::Client::builder()`, applying `oauth()`, setting `token_dir("~/.scorpio-analyst/github_copilot")`, optionally applying a custom `base_url`, and never requiring a Scorpio-managed API key.
- Add a Xiaomi MiMo branch that requires a configured API key and uses `xiaomimimo::Client::builder().api_key(...).base_url(...).build()` when `base_url` is present, otherwise `xiaomimimo::Client::new(...)`.

This file remains the only place where provider-name strings become concrete rig clients.

### Agent construction

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add Copilot and Xiaomi MiMo to the internal dispatch enum and type aliases.
- Extend `build_agent_inner(...)` to construct provider-backed agents for both clients through the same `CompletionClient` pattern used elsewhere.
- Preserve Scorpio's current provider-agnostic `LlmAgent` public surface.

The intent is that every downstream agent continues to depend on Scorpio's wrapper, not on provider-specific `rig` types.

### Rate limiting

`crates/scorpio-core/src/rate_limit.rs`

- Add Copilot and Xiaomi MiMo to `ProviderRateLimiters::from_config(...)`.
- Extend the provider-to-RPM mapping helpers and tests.
- Use conservative defaults for both providers so the first implementation does not assume premium quotas.

This keeps rate limiting uniform even though Copilot auth differs from the keyed providers.

### Setup wizard provider groups

`crates/scorpio-cli/src/cli/setup/steps.rs`

The current setup code uses one provider list for both key collection and provider routing. That no longer works once Copilot becomes routable without a key.

Split the setup surface into two explicit groups:

- keyed providers: OpenAI, Anthropic, Gemini, OpenRouter, DeepSeek, Xiaomi MiMo
- routable providers: Copilot plus every keyed provider that currently has a saved key

The consequences are:

- Step 3 continues to collect secrets only for keyed providers.
- Step 3 must include Xiaomi MiMo in provider-key prompts, validation, helper tables, and redaction tests.
- Step 3 must not force the user to configure an API key just to continue with a Copilot-only setup.
- Step 4 routing must always offer Copilot, even when no keyed provider is configured.
- Provider prompt defaulting should preserve any saved provider when it remains eligible.
- To avoid surprising defaults, Copilot should be appended to the routing menu rather than becoming the implicit first choice when keyed providers are available.

This is the only meaningful setup UX restructuring required by Copilot's auth model.

### Setup model selection

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

Extend the existing model-selection flow without changing its provider-first mental model.

Copilot behavior:

- Return a `Listed(...)` discovery outcome from a static curated list.
- Include rig's known completion models that fit Scorpio's usage, such as `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-4.1-mini`, `gpt-4.1-nano`, `gpt-5.3-codex`, `gpt-5.1-codex`, `claude-sonnet-4`, `claude-3.5-sonnet`, `gemini-2.0-flash-001`, and `o3-mini`.
- Append `Enter model manually` as the last option.

Xiaomi MiMo behavior:

- Reuse the existing discovery path when no custom base URL is configured.
- Call `list_models()` through `rig::providers::xiaomimimo::Client`.
- Preserve provider-returned order and all IDs as-is.
- Degrade to manual entry when a custom base URL is set, discovery fails, or listing returns nothing.

The saved-model and manual-entry behavior should remain aligned with the current setup UX.

### Discovery seam in `scorpio-core`

`crates/scorpio-core/src/providers/factory/discovery.rs`

Extend the existing discovery module rather than introducing a second setup-only provider-selection path.

Add provider-specific discovery behavior:

- `ProviderId::Copilot` returns `ModelDiscoveryOutcome::Listed(curated_models)` with no network request
- `ProviderId::XiaomiMimo` calls `list_models()` when a key is present and `base_url` is absent
- `ProviderId::XiaomiMimo` returns manual fallback when `base_url` is configured

This keeps all setup-time model discovery rules inside the core provider facade.

### Public docs

`README.md`

- Re-add Copilot to the supported-provider documentation, but describe it as rig-native OAuth-backed support rather than the deleted custom runtime.
- Add Xiaomi MiMo to supported-provider lists and setup guidance.
- Update any provider examples or supported-provider enumerations to include both new providers.

`.env.example`

- Add `SCORPIO_XIAOMIMIMO_API_KEY=` alongside the other LLM provider keys.
- Do not add a Copilot env key.

## Runtime flow

1. The user selects quick-thinking and deep-thinking providers through setup or direct config.
2. `Config::load_*` merges provider names, provider settings, and available secrets into the effective runtime config.
3. `create_completion_model(...)` resolves the provider name for each tier.
4. When the provider is `copilot`, the factory builds a rig Copilot client using OAuth and the Scorpio-owned token directory.
5. When the provider is `xiaomimimo`, the factory builds a keyed Xiaomi MiMo client and attaches the provider rate limiter.
6. `build_agent(...)` wraps the concrete client in Scorpio's unified `LlmAgent` interface.
7. Prompt, chat, retry, and rate-limit execution continue to flow through the existing provider-agnostic wrappers.

This keeps the new providers normal tier-selectable options rather than introducing provider-specific call paths deeper in the app.

## Error handling

### Config-time behavior

- `copilot` and `xiaomimimo` must validate as legal provider names anywhere runtime config currently validates provider names.
- Xiaomi MiMo missing-key failures should continue to surface as `TradingError::Config` with the hint `SCORPIO_XIAOMIMIMO_API_KEY`.
- Copilot must not use the generic missing-key error path.

### Auth/runtime behavior

- Copilot client construction should succeed without an API key.
- If rig needs OAuth/device authorization, the setup health check or first runtime request should surface that auth flow naturally.
- Copilot auth and runtime failures should pass through Scorpio's existing provider error-sanitization helpers so tokens or auth artifacts are not leaked.
- Xiaomi MiMo runtime failures should flow through the existing retry and sanitization layers like the other keyed providers.

### Compatibility behavior

- A previously saved `copilot` route from the temporary-removal window should become valid again.
- Scorpio should no longer emit the friendly "Copilot was removed" guidance once Copilot support is restored.

## Testing

### Unit and module coverage

`crates/scorpio-core/src/config.rs`

- Add provider-name validation coverage for `copilot` and `xiaomimimo`.
- Add env-loading coverage for `SCORPIO_XIAOMIMIMO_API_KEY`.
- Update warning-path tests so Copilot-only routing does not trigger misleading "no API key" diagnostics.

`crates/scorpio-core/src/settings.rs`

- Add round-trip coverage for Xiaomi MiMo secrets and both providers' non-secret settings.
- Extend debug-redaction coverage so the Xiaomi MiMo secret never appears in plain text.

`crates/scorpio-core/src/providers/factory/client.rs`

- Add factory construction tests for Copilot success without a key.
- Add Xiaomi MiMo success, missing-key failure, and base-url override tests.
- Add validation coverage for the new provider-name branches.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add agent-construction tests proving both new provider variants build correctly.

`crates/scorpio-core/src/providers/factory/discovery.rs`

- Add tests proving Copilot returns the curated static list.
- Add tests proving Xiaomi MiMo discovery maps model IDs correctly and falls back to manual entry when required.

`crates/scorpio-core/src/rate_limit.rs`

- Extend limiter-construction tests to cover Copilot and Xiaomi MiMo.

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Add tests for the split keyed-provider vs routable-provider logic.
- Add Xiaomi MiMo key-prompt coverage.
- Add Copilot routing eligibility coverage when no keys are configured.

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

- Add Copilot static-model menu coverage.
- Add Xiaomi MiMo listing/manual fallback coverage.
- Preserve saved-provider and saved-model defaulting tests across the expanded provider set.

### Verification commands

Implementation is only complete after all required repo checks pass in order:

1. `cargo fmt -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## Implementation boundaries

- Keep changes concentrated in the provider, config, setup, and docs seams already listed above.
- Prefer extending existing enums, helper tables, and setup flows over introducing new abstraction layers.
- Do not recreate a custom Copilot provider or any Copilot-specific transport layer outside rig.
- Do not split Xiaomi MiMo into multiple Scorpio provider IDs unless runtime evidence later proves it necessary.
- Keep all downstream agent/task code provider-agnostic.
