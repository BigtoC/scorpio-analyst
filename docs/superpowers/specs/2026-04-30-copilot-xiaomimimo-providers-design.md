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

| Decision                       | Choice                                                                                                                | Rationale                                                                                                                |
|--------------------------------|-----------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------|
| Copilot integration            | Native `rig::providers::copilot` client                                                                               | Reuses upstream auth/runtime behavior and avoids reviving deleted custom runtime code                                    |
| Xiaomi MiMo integration        | Native `rig::providers::xiaomimimo::Client`                                                                           | Matches upstream provider identity and preserves model-listing support                                                   |
| Xiaomi MiMo provider shape     | One provider: `xiaomimimo`                                                                                            | Approved user scope; avoids exposing API-dialect details in Scorpio config                                               |
| Copilot auth mode              | OAuth/device flow with an explicit setup-vs-runtime context boundary                                                  | Approved user scope; no Scorpio-managed `SCORPIO_COPILOT_API_KEY`, and auth prompting must stay predictable              |
| Copilot token cache location   | Derived absolute path under `~/.scorpio-analyst/github_copilot/` using Scorpio's existing config-dir resolution       | Keeps Scorpio-owned auth state under the project config root without relying on a literal `~` string                     |
| Copilot setup routing          | Copilot is always selectable in step 4 and never appears in step 3 key entry                                          | Smallest change that makes one non-keyed provider routable                                                               |
| Copilot setup models           | Small curated starter list plus manual entry                                                                          | Meets the approved static-list requirement without turning setup into a model-catalog maintenance task                   |
| Xiaomi MiMo setup models       | Live listing when `base_url` is absent; manual fallback otherwise                                                     | Matches approved user scope and current setup behavior for custom base URLs                                              |
| Xiaomi MiMo custom host policy | First slice keeps `base_url` support, but only for trusted HTTPS endpoints and with explicit trust-boundary messaging | Existing providers already support `base_url`, but this provider must document that prompts and API keys go to that host |
| Copilot-only setup path        | Explicit step-3 bypass when no keyed provider is effectively configured                                               | Makes Copilot-only setup possible in the current wizard flow                                                             |
| Keyed-provider eligibility     | Use the effective merged provider config, not only saved file secrets                                                 | Keeps setup aligned with runtime precedence, including env-provided credentials                                          |
| Copilot auth trigger           | Only interactive setup health checks may start device flow                                                            | Avoids surprise auth prompts in runtime or non-interactive contexts                                                      |
| Copilot endpoint override      | Unsupported in the first slice                                                                                        | Avoids widening the OAuth token trust boundary to arbitrary custom hosts                                                 |
| Saved Copilot compatibility    | Treat saved `copilot` routes as valid again                                                                           | The recent stale-Copilot recovery path should disappear once Copilot is supported again                                  |
| Default RPM posture            | Conservative defaults for both new providers                                                                          | Avoid optimistic quota assumptions for newly added providers                                                             |

## Architecture

The existing provider layer in `crates/scorpio-core/src/providers/` remains the sole runtime integration seam.

The target architecture is:

1. `ProviderId` gains `Copilot` and `XiaomiMimo`.
2. Runtime config accepts the canonical provider names `copilot` and `xiaomimimo`.
3. `ProvidersConfig` gains `[providers.copilot]` and `[providers.xiaomimimo]` sections, still reusing `ProviderSettings`; for Copilot, `rpm` is used while `api_key` remains unused and `base_url` is unsupported in this first slice.
4. `PartialConfig` gains Xiaomi MiMo secret support plus non-secret Xiaomi MiMo `base_url` / `rpm` fields and a Copilot `rpm` field; the on-disk representation remains nested `[providers.*]` tables.
5. `create_completion_model(...)` resolves `copilot` into a rig Copilot client and `xiaomimimo` into a rig Xiaomi MiMo client.
6. `build_agent(...)` and the retry helpers continue to present Scorpio's provider-agnostic LLM interface.
7. `scorpio setup` keeps the current provider-first flow, but treats Copilot as a targeted exception: it is always routable and never appears in keyed-provider prompts.
8. Step-4 routing for keyed providers uses the effective merged provider config (saved file + env), not only file-backed secrets.
9. Setup-time model discovery loads provider settings independently of `[llm]` routing and merges the current in-memory `PartialConfig` with file and env overrides, so first-run Xiaomi MiMo listing works before quick/deep routing has been chosen and before setup has been saved.
10. Copilot authorization behavior is context-sensitive: interactive setup verification may enter device flow, while normal runtime must only use already-cached auth material and otherwise fail with guidance.
11. Copilot model selection is setup-only static data; Xiaomi MiMo model selection is provider-backed when listing is available.

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
- Reuse `ProviderSettings` for `providers.copilot`, but make the contract explicit: `api_key` is permanently unused by Scorpio, and `base_url` is rejected with a config error in this slice rather than forwarded to rig.
- Extend helper lookups like `settings_for(...)`, `base_url_for(...)`, `rpm_for(...)`, and `api_key_for(...)`.
- Add Xiaomi MiMo env-secret loading via `SCORPIO_XIAOMIMIMO_API_KEY`.
- Do not add a Scorpio-specific Copilot API-key env contract.
- Adjust the current "no LLM provider API key found" warning path so Copilot-only routing does not produce misleading diagnostics.

The important config rule is:

- Xiaomi MiMo behaves like the existing keyed providers.
- Copilot behaves like a configured provider without requiring Scorpio-managed secret storage.

### Persisted setup boundary

`crates/scorpio-core/src/settings.rs`

- Add `xiaomimimo_api_key: Option<String>` to `PartialConfig`; continue converting it through the existing `UserConfigFile` / `UserConfigProviders` pipeline rather than introducing a new flat persisted override shape.
- Add `copilot_rpm`, `xiaomimimo_base_url`, and `xiaomimimo_rpm` as non-secret internal `PartialConfig` fields.
- Do not add `copilot_api_key`.
- Keep the canonical on-disk shape nested under `[providers.copilot]` and `[providers.xiaomimimo]` through the existing `UserConfigProviders` path; do not reintroduce legacy-style flat persisted provider override keys for the new providers.
- Extend `UserConfigProviders`, `From<UserConfigFile> for PartialConfig`, `From<&PartialConfig> for UserConfigFile`, and `config.rs::partial_to_nested_toml_non_secrets(...)` so the new provider overrides round-trip through the current config pipeline.
- Extend the redacted `Debug` implementation and the round-trip/load/save tests accordingly.
- Derive the Copilot token directory from Scorpio's existing config-dir resolution (`settings::user_config_path()` and its parent directory, or a thin helper built on that path) rather than passing a literal `~` string to rig.
- Extend `Config::load_effective_providers_config_from_user_path(...)`, `apply_partial_provider_secrets(...)`, and `apply_provider_secret_env_overrides(...)` so setup uses the same merged provider view as runtime.
- Treat env-derived secrets as read-only inputs for setup eligibility, discovery, and routing. They must never be copied back into `PartialConfig` during save or written to `config.toml` unless the user explicitly entered them in setup.

### Provider construction

`crates/scorpio-core/src/providers/factory/client.rs`

- Import `rig::providers::{copilot, xiaomimimo}`.
- Extend `ProviderClient` with `Copilot(copilot::Client)` and `XiaomiMimo(xiaomimimo::Client)`.
- Extend `validate_provider_id(...)` so `copilot` and `xiaomimimo` resolve cleanly.
- Add a small explicit Copilot auth context at the Scorpio seam, for example `CopilotAuthMode::{InteractiveSetup, NonInteractiveRuntime}`.
- Thread that mode through `create_completion_model(...)` and the first Copilot request path so setup and runtime do not have to fork the whole provider factory.
- Add a Copilot client-construction branch using `copilot::Client::builder()`, setting a resolved absolute token directory under Scorpio's config root, and enabling `oauth()` only for the interactive setup mode.
- In runtime mode, permit only cached/reusable Copilot auth state that Scorpio can validate as equal to or narrower than the approved Copilot-inference privilege allowlist. If the cache is missing, expired, or broader than allowed, fail with sanitized guidance instead of beginning device flow.
- Make the allowed interactive seam explicit: only the final setup verification step (`step5_health_check`) may construct Copilot with `InteractiveSetup`. Runtime call sites such as `Config::is_analysis_ready(...)`, `AnalysisRuntime` initialization, and normal analyze execution use `NonInteractiveRuntime`.
- Reject `providers.copilot.base_url` with a config error in this first slice instead of forwarding OAuth traffic to an arbitrary custom host.
- Resolve and create the Copilot token directory before handing it to rig, use owner-only directory permissions on Unix, and surface a sanitized config/auth error if the path cannot be resolved or created.
- Treat the Copilot token directory as rig-managed secret state: Scorpio must not mirror tokens into config files, env vars, or logs. Cache reset / re-auth for this slice is deleting that directory and rerunning setup.
- Add a Xiaomi MiMo branch that requires a configured API key and uses `xiaomimimo::Client::builder().api_key(...).base_url(...).build()` when `base_url` is present, otherwise `xiaomimimo::Client::new(...)`.
- For Xiaomi MiMo `base_url`, parse and validate the URL structurally. Require `https://` by default. Plain `http://` is allowed only for exact loopback targets used in explicit local development. Reject ambiguous or unsafe forms such as userinfo-based hosts (`127.0.0.1@evil.com`) or lookalikes like `localhost.evil.com`. The setup/docs path must warn that prompts and API keys will be sent to that configured host.

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

### Setup wizard flow

`crates/scorpio-cli/src/cli/setup/steps.rs`

The current setup code is provider-first and should stay that way. Copilot is the only non-keyed provider, so the design should treat it as a targeted exception rather than introducing a broad new provider taxonomy.

The required setup behavior is:

- Step 3 key entry continues to operate only on keyed providers: OpenAI, Anthropic, Gemini, OpenRouter, DeepSeek, and Xiaomi MiMo.
- Xiaomi MiMo must be added everywhere Step 3 already handles keyed providers: prompts, validation, helper tables, persistence, and redaction tests.
- If no keyed provider is effectively configured via saved config or env, Step 3 must offer an explicit "continue with Copilot only" path before entering the key-entry loop.
- Choosing that path leaves keyed-provider secrets unset and proceeds to Step 4.
- Step 4 routing must always include Copilot.
- Step 4 must include additional keyed providers based on the effective merged provider config (saved file + env), not only `PartialConfig`'s file-backed secret fields.
- When keyed providers are available, append Copilot after them so existing default-selection behavior stays stable and Copilot does not become the implicit first choice.
- Provider prompt defaulting should continue to preserve any saved provider when it remains eligible.
- Using env-derived keyed-provider credentials in setup must not persist those secrets back into the saved user config unless the user explicitly typed them during this setup run.

### Setup model selection

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

Extend the existing model-selection flow without changing its provider-first mental model.

Copilot behavior:

- Return a `Listed(...)` discovery outcome from a static curated list.
- Keep that list intentionally small in the first slice, for example `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `gpt-5.1-codex`, `claude-sonnet-4`, `gemini-2.0-flash-001`, and `o3-mini`.
- Append `Enter model manually` as the last option.
- If a saved Copilot model is not in the curated list, default to manual entry and prefill the saved value rather than remapping it automatically.

Xiaomi MiMo behavior:

- Reuse the existing discovery path when no custom base URL is configured.
- Call `list_models()` through `rig::providers::xiaomimimo::Client`.
- Preserve provider-returned order, but validate and escape provider-supplied model IDs before display or persistence so control characters and pathological strings cannot reach the terminal or config file.
- Degrade to manual entry when a custom base URL is set, discovery fails, or listing returns nothing.

The saved-model and manual-entry behavior should remain aligned with the current setup UX.

### Discovery seam in `scorpio-core`

`crates/scorpio-core/src/providers/factory/discovery.rs`

Extend the existing discovery module rather than introducing a second setup-only provider-selection path.

Add provider-specific discovery behavior:

- `ProviderId::Copilot` returns `ModelDiscoveryOutcome::Listed(curated_models)` with no network request
- `ProviderId::XiaomiMimo` calls `list_models()` when a key is present and `base_url` is absent
- `ProviderId::XiaomiMimo` returns manual fallback when `base_url` is configured
- Setup-time discovery must not bootstrap through `Config::load_effective_runtime(partial.clone())`; it must use a provider-only load path that merges the current in-memory `PartialConfig` with file and env overrides so first-run discovery works before `[llm]` routing exists and before setup state is persisted.

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
- `providers.copilot.base_url` must fail fast with a config error in this first slice.

### Auth/runtime behavior

- Copilot client construction should succeed without an API key.
- Only interactive setup health checks may trigger Copilot device flow in this first slice.
- The control seam for that rule is the explicit Copilot auth mode passed into the Copilot construction / first-use path.
- `step5_health_check` is the only approved interactive verification seam for Copilot auth in setup.
- The approved Copilot bootstrap privilege allowlist for this slice is the current rig-native GitHub device-flow request scope `read:user` only.
- "Equal to or narrower than the allowlist" therefore means the bootstrap token was minted through that exact `read:user` flow, and any future upstream scope metadata surfaced to Scorpio must not broaden it beyond `read:user`.
- The authoritative comparison source is Scorpio's own Copilot token directory under `~/.scorpio-analyst/github_copilot/`, specifically the rig-managed bootstrap token cache plus the paired `api-key.json` metadata that binds the Copilot API token to a bootstrap-token fingerprint.
- Scorpio must also own a minimal identity-binding record outside the rig-managed cache, stored under the same config root with owner-only permissions, containing the confirmed GitHub identity Scorpio accepted for this Copilot setup (for example numeric account ID plus login).
- Before starting Copilot device flow in `step5_health_check`, setup must show the expected OAuth privilege boundary and require explicit user confirmation.
- That same setup confirmation step must also surface the GitHub identity Scorpio expects to authorize once it can be determined from the `read:user` bootstrap flow.
- If Scorpio cannot determine the requested privilege boundary from upstream behavior, or if it appears broader than the approved Copilot-inference allowlist, setup must abort instead of silently proceeding into device flow.
- After device flow completes and rig writes auth state, `step5_health_check` must immediately re-open the Scorpio-owned Copilot token directory and re-run the same validation checks before treating the authorization as successful or issuing any inference call.
- Fresh Copilot auth is valid only when Scorpio can verify all of the following from the just-written cache: the bootstrap token came from the expected `read:user` flow, the rig metadata binds the Copilot API token to that bootstrap-token fingerprint, and the confirmed GitHub identity matches the one surfaced during setup consent. Only after that check succeeds may Scorpio write or refresh its own identity-binding record.
- Cached Copilot auth must be validated against that same allowlist before reuse in either setup or runtime. Scorpio should accept cached auth only when it can verify all of the following: the cache lives under Scorpio's managed Copilot token directory, the rig metadata binds the Copilot API token to the cached bootstrap token fingerprint, and no surfaced upstream privilege metadata broadens the bootstrap grant beyond `read:user`.
- Cached Copilot auth must also be bound to that Scorpio-owned identity reference. Runtime or setup reuse is valid only when the freshly revalidated bootstrap-path identity matches the stored Scorpio-owned identity-binding record.
- If Scorpio cannot prove a cached grant is within the allowlist, if the metadata binding is missing, if the identity-binding record is missing, unreadable, or inconsistent with the freshly revalidated identity, or if a future rig/upstream change stops exposing the fields Scorpio depends on for this comparison, it must fail closed, refuse reuse, ask the user to clear the Copilot cache, and require a fresh confirmed device-flow authorization.
- Non-interactive runtime paths should fail fast with sanitized, actionable guidance rather than initiating device flow automatically.
- Display the Copilot verification URI and user code only to the interactive terminal path, never to structured logs or sanitized error strings.
- Copilot auth and runtime failures should pass through Scorpio's error-sanitization helpers, extended to redact GitHub OAuth token prefixes (for example `ghu_`, `gho_`, `ghr_`, `github_pat_`), bearer tokens, and device/user codes.
- Copilot support must document and verify the least-privilege expectation of the upstream OAuth flow; if upstream exposes broader GitHub token access than required for Copilot inference, Scorpio should surface that as a documented risk and avoid broadening it further.
- If the Copilot token directory cannot be resolved or created, fail with a sanitized actionable error instead of silently falling back to rig's global default location.
- Xiaomi MiMo runtime failures should flow through the existing retry and sanitization layers like the other keyed providers.

### Provider trust boundaries

- `providers.copilot.base_url` is unsupported because OAuth-backed traffic must not be redirected to arbitrary hosts in this slice.
- `providers.xiaomimimo.base_url` remains supported, but Scorpio must treat it as an advanced trusted-host override.
- Setup text and docs must warn that Xiaomi MiMo prompts, responses, and API keys are sent to the configured host.
- Non-HTTPS Xiaomi MiMo endpoints should be rejected except explicit loopback/local-development cases validated by structural URL parsing.

### Compatibility behavior

- A previously saved `copilot` route from the temporary-removal window should become valid again.
- Saved Copilot models outside the curated setup list should be preserved as manual values during setup rather than automatically remapped.
- Scorpio should no longer emit the friendly "Copilot was removed" guidance once Copilot support is restored.

## Testing

### Unit and module coverage

`crates/scorpio-core/src/config.rs`

- Add provider-name validation coverage for `copilot` and `xiaomimimo`.
- Add env-loading coverage for `SCORPIO_XIAOMIMIMO_API_KEY`.
- Update warning-path tests so Copilot-only routing does not trigger misleading "no API key" diagnostics.
- Extend provider-only config-loading coverage for `load_effective_providers_config_from_user_path(...)`, including env-backed keyed providers and the new nested provider sections.
- Add coverage that env-backed Xiaomi MiMo credentials participate in setup eligibility/discovery without being written back to saved config.

`crates/scorpio-core/src/settings.rs`

- Add round-trip coverage for Xiaomi MiMo secrets and both providers' non-secret settings.
- Extend debug-redaction coverage so the Xiaomi MiMo secret never appears in plain text.
- Add coverage that the new provider overrides still serialize back into nested `[providers.*]` tables.

`crates/scorpio-core/src/providers/factory/client.rs`

- Add factory construction tests for Copilot success without a key.
- Add Xiaomi MiMo success, missing-key failure, and base-url override tests.
- Add validation coverage for the new provider-name branches.
- Add Copilot token-dir resolution / creation failure coverage and Copilot `base_url` rejection coverage.
- Add Copilot auth-mode coverage proving setup may enter interactive auth while runtime refuses to start device flow.
- Add coverage that setup requires explicit user confirmation before entering Copilot device flow and aborts when the upstream OAuth privilege boundary is unknown or broader than allowed.
- Add coverage that `step5_health_check` re-validates newly written Copilot auth after device flow completes before reporting success.
- Add coverage that cached Copilot auth is rejected when its privilege boundary is unknown or broader than the approved allowlist.
- Add coverage that the approved allowlist is exactly the rig-native `read:user` bootstrap scope, and that cached Copilot auth is rejected when the bootstrap-token fingerprint binding is missing or unverifiable.
- Add coverage that cached Copilot auth is rejected when the confirmed GitHub identity is missing, changed unexpectedly, or cannot be bound to the trusted bootstrap path.
- Add coverage that Scorpio's own identity-binding record is written only after successful post-device-flow validation and that cached auth is rejected when that record is missing, unreadable, or inconsistent.
- Add Xiaomi MiMo `base_url` validation coverage for HTTPS, loopback exceptions, and rejected insecure remote hosts.
- Add caller-mapping coverage or targeted integration tests proving `step5_health_check` uses the interactive Copilot auth mode while runtime readiness paths use the non-interactive mode.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add agent-construction tests proving both new provider variants build correctly.

`crates/scorpio-core/src/providers/factory/discovery.rs`

- Add tests proving Copilot returns the curated static list.
- Add tests proving Xiaomi MiMo discovery maps model IDs correctly and falls back to manual entry when required.
- Add first-run discovery coverage proving Xiaomi MiMo listing works before quick/deep routing exists.

`crates/scorpio-core/src/providers/factory/error.rs`

- Add redaction coverage for GitHub OAuth token prefixes, bearer tokens, and device-flow artifacts used by Copilot.

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

- Add coverage that provider-returned Xiaomi MiMo model IDs are escaped or rejected before terminal display and persistence.

`crates/scorpio-core/src/rate_limit.rs`

- Extend limiter-construction tests to cover Copilot and Xiaomi MiMo.

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Add tests for the explicit Copilot-only step-3 bypass.
- Add Xiaomi MiMo key-prompt coverage.
- Add Copilot routing eligibility coverage when no keys are configured.
- Add routing eligibility coverage for env-only keyed providers.

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

- Add Copilot static-model menu coverage.
- Add Xiaomi MiMo listing/manual fallback coverage.
- Add coverage that saved-but-unlisted Copilot models default to manual entry with prefilled text.
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
