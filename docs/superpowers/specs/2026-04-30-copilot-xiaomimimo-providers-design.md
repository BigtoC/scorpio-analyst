# Design — add native GitHub Copilot and Xiaomi MiMo providers

**Date:** 2026-04-30
**Author:** brainstorming session with BigtoC
**Status:** Revised — security model and rig API gaps addressed; ready for implementation plan

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

**Effective Merged Provider Config** (used throughout this document): The union of Scorpio-saved provider settings (`PartialConfig`'s provider secrets, base URLs, and RPM limits) and environment-variable overrides (`SCORPIO_*_API_KEY`, `SCORPIO__PROVIDERS__*` env vars). Does *not* include `[llm]` tier routing decisions; those are independent. Used for setup eligibility, runtime client construction, and rate limiting.

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
- Adjust the current "no LLM provider API key found" warning path so Copilot-only routing does not produce misleading diagnostics. In `validate()` (config.rs:569–592), check whether both `llm.quick_thinking_provider` and `llm.deep_thinking_provider` are `"copilot"` and skip or replace the no-key warning in that case. Audit all call sites of `has_any_llm_key()` and `is_analysis_ready()` to ensure none emit a false missing-key warning for Copilot-only configs.

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
- Add a small explicit Copilot auth context at the Scorpio seam: `CopilotAuthMode::{InteractiveSetup, NonInteractiveRuntime}`.
- Thread that mode through `create_completion_model(...)` and the first Copilot request path so setup and runtime do not have to fork the whole provider factory.
- **InteractiveSetup path:** build the client using `copilot::Client::builder().oauth().token_dir(scorpio_token_dir).build()`. The `.oauth()` call enables device flow. This path is exclusively for `step5_health_check`.
- **NonInteractiveRuntime path:** before constructing any Copilot client, check for the existence of the token cache files under Scorpio's managed token directory. If the expected files are absent, return `TradingError::Config` with guidance to run `scorpio setup` — do not attempt client construction. If the files exist, construct the client using `copilot::Client::builder().oauth().token_dir(scorpio_token_dir).on_device_code(|_| { /* no-op: device flow is not allowed in runtime */ }).build()`. This no-op handler ensures that if the cache is stale and rig internally attempts device flow, it will receive no user-code display and the resulting auth failure is caught and converted to a `TradingError::Config`.
- **Never call `copilot::Client::from_env()`** in any Scorpio code path. rig's `from_env()` checks `GITHUB_COPILOT_API_KEY`, `COPILOT_GITHUB_ACCESS_TOKEN`, and `GITHUB_TOKEN` env vars, which would bypass Scorpio's device-flow gate and token directory isolation. Always construct via the builder with an explicit `token_dir`.
- Make the allowed interactive seam explicit: only the final setup verification step (`step5_health_check`) may construct Copilot with `InteractiveSetup`. All other call sites (`Config::is_analysis_ready(...)`, `AnalysisRuntime` initialization, normal analyze execution) use `NonInteractiveRuntime`.
- Reject `providers.copilot.base_url` with a config error in this first slice instead of forwarding OAuth traffic to an arbitrary custom host.
- Resolve and create the Copilot token directory before handing it to rig, use owner-only directory permissions (`0o700`) on Unix, and surface a sanitized config/auth error if the path cannot be resolved or created. **The `.token_dir()` call on the builder is mandatory on every code path.** rig's default token dir is `$XDG_CONFIG_HOME/github_copilot` (shared with VS Code, JetBrains, etc.) — omitting `.token_dir()` silently uses the system-wide cache, which breaks Scorpio's auth isolation and makes cache resets ineffective. Add a test asserting that every Copilot client construction path in Scorpio passes a token directory argument.
- Before reading cached tokens on any reuse (setup or runtime), verify that the token directory is owned by the current effective user and has at most `0o700` permissions on Unix. If the check fails, treat it as a cache-miss and fail with guidance rather than proceeding.
- Treat the Copilot token directory as rig-managed secret state: Scorpio must not mirror tokens into config files, env vars, or logs. Cache reset / re-auth for this slice is deleting that directory and rerunning setup.
- Add a Xiaomi MiMo branch that requires a configured API key and uses `xiaomimimo::Client::builder().api_key(...).base_url(...).build()` when `base_url` is present, otherwise `xiaomimimo::Client::new(...)`.
- For Xiaomi MiMo `base_url`, parse and validate the URL using the `url` crate (never string-prefix or contains checks). Require `https://` by default. Plain `http://` is allowed only when the parsed host component is an exact member of the loopback allowlist `{127.0.0.1, ::1, localhost}` — no other hosts. Reject any URL where the parsed `userinfo` component is non-empty (e.g. `127.0.0.1@evil.com` parses as host `evil.com`). Reject empty `base_url` values (`Some("")` or `Some("  ")`) the same as `None`. The setup/docs path must warn that prompts and API keys will be sent to that configured host.

This file remains the only place where provider-name strings become concrete rig clients.

### Agent construction

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add Copilot and Xiaomi MiMo to the internal dispatch enum and type aliases. The concrete types are:
  - `type CopilotModel = rig::providers::copilot::CompletionModel<reqwest::Client>`
  - `type XiaomiMimoModel = rig::providers::openai::completion::GenericCompletionModel<rig::providers::xiaomimimo::XiaomiMimoExt, reqwest::Client>`
- Extend `build_agent_inner(...)` to construct provider-backed agents for both clients through the same `CompletionClient` pattern used elsewhere.
- Confirm whether Copilot or Xiaomi MiMo require `.max_tokens(N)` during agent construction (Anthropic requires `.max_tokens(4096)`); specify the requirement explicitly so implementers do not have to discover it at runtime.
- Preserve Scorpio's current provider-agnostic `LlmAgent` public surface.
- Extend token usage tracking to handle `CopilotCompletionResponse::Chat` (uses `prompt_tokens + total_tokens` from `openai::completion::Usage`) and `CopilotCompletionResponse::Responses` (uses `input_tokens + output_tokens` from `ResponsesUsage`). Specify how both map to Scorpio's `TokenUsageTracker` fields.

The intent is that every downstream agent continues to depend on Scorpio's wrapper, not on provider-specific `rig` types.

### Rate limiting

`crates/scorpio-core/src/rate_limit.rs`

- Add Copilot and Xiaomi MiMo to `ProviderRateLimiters::from_config(...)`.
- Extend the provider-to-RPM mapping helpers and tests.
- Use conservative defaults for both providers so the first implementation does not assume premium quotas:
  - **Copilot:** 30 requests/min (conservative for device-flow-backed, non-premium auth)
  - **Xiaomi MiMo:** 50 requests/min (pending provider documentation; increase if observed limits are higher)

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
- **`validate_step3_result` and `providers_with_keys` must both be updated.** `validate_step3_result` currently errors when all five keyed provider keys are `None` — it must also pass when Copilot-only was selected (tracked via a `copilot_only: bool` wizard-state flag or equivalent). `providers_with_keys` returns only providers with a non-None key field; step 4 routing must instead query a separate `eligible_routing_providers(partial, copilot_only_selected)` function that always appends `ProviderId::Copilot`. The `WIZARD_PROVIDERS` constant must be split into `KEYED_WIZARD_PROVIDERS` (step 3, keyed providers only) and a step 4 routing list.
- Provider prompt defaulting should continue to preserve any saved provider when it remains eligible.
- Using env-derived keyed-provider credentials in setup must not persist those secrets back into the saved user config unless the user explicitly typed them during this setup run.

### Setup model selection

`crates/scorpio-cli/src/cli/setup/model_selection.rs`

Extend the existing model-selection flow without changing its provider-first mental model.

Copilot behavior:

- Return a `Listed(...)` discovery outcome from a static curated list.
- Keep that list intentionally small in the first slice, for example `gpt-4o`, `gpt-4o-mini`, `gpt-4.1`, `claude-sonnet-4`, `gemini-2.0-flash-001`, and `o3-mini`. **Do not include `gpt-5.1-codex` or other Codex-class models in the first slice:** rig routes any model whose lowercase name contains `codex` to the `/responses` endpoint (`CompletionRoute::Responses`) instead of `/chat/completions`, which uses a different request/response shape (`ResponsesRequest`, `ResponsesUsage`) that may interact differently with Scorpio's structured-output and tool-calling paths. Codex models can be added to the curated list in a follow-up slice after verifying compatibility.
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

**Copilot auth model**

rig-core 0.36.0 does not expose the OAuth scope of a cached grant, and the `bootstrap_token_fingerprint` stored in `api-key.json` is computed by `DefaultHasher` (process-randomized, non-cryptographic, not cross-process verifiable). Scorpio's security model therefore relies on a live GitHub identity call, not on internal rig metadata.

- Copilot client construction must succeed without a Scorpio-managed API key.
- Only `step5_health_check` may trigger Copilot device flow.
- The control seam is `CopilotAuthMode::{InteractiveSetup, NonInteractiveRuntime}` threaded into the factory; see the Provider construction section for the concrete builder approaches for each mode.

**Device flow consent (InteractiveSetup / `step5_health_check` only)**

1. Before initiating device flow, display the expected OAuth privilege boundary (`read:user` scope) and require explicit user confirmation.
2. Initiate rig's device flow. Display the verification URI and user code to the interactive terminal only — never to structured logs or error strings.
3. After device flow completes and rig writes auth state, immediately call `GET https://api.github.com/user` using the access token rig cached, and read the `X-OAuth-Scopes` response header.
4. Validate: the `X-OAuth-Scopes` header must include `read:user` and must not include any scope broader than Copilot inference requires (e.g., `repo`, `write:*`, `admin:*`). If the scope is absent, unreadable, or broader than allowed, abort and instruct the user to clear the Copilot cache and rerun setup.
5. Record the numeric GitHub account ID and login from the `GET /user` response body into Scorpio's identity-binding record (stored at `<config_root>/github_copilot/scorpio-identity.json` with `0o600` permissions). The numeric account ID is mandatory — the login is stored for display only and must not be used as the primary identity key.
6. Only after steps 4 and 5 succeed may setup report success and issue any inference call.

**Cached auth reuse (setup and runtime)**

Before reusing cached Copilot credentials:

1. Verify the token directory is owned by the current effective user and has at most `0o700` permissions. Fail closed if not.
2. Verify `scorpio-identity.json` exists, is readable, and contains a numeric account ID.
3. Call `GET https://api.github.com/user` with the cached access token to re-confirm the live identity. The returned numeric account ID must match the stored record. If the token is expired or the identity does not match, fail closed, ask the user to clear the Copilot cache, and require a fresh setup run.
4. Read the `X-OAuth-Scopes` header from the same response and apply the same scope validation as step 4 above.

Note: GitHub Enterprise Copilot installations may redirect inference traffic to a custom endpoint via the `api` field in rig's `api-key.json` — this is a GitHub-controlled redirect (not user-configurable) and is acceptable. The trust boundary restriction on `providers.copilot.base_url` is specifically about user-specified host overrides, not GitHub Enterprise-managed redirects.

**Error handling**

- Non-interactive runtime paths must fail fast with sanitized, actionable guidance — never initiate device flow.
- If the Copilot token directory cannot be resolved or created, fail with a sanitized actionable error (never fall back to rig's global default `~/.config/github_copilot`).
- Copilot auth and runtime failures must pass through Scorpio's error-sanitization helpers, extended to redact GitHub OAuth token prefixes (`ghu_`, `gho_`, `ghr_`, `github_pat_`), bearer tokens, and device/user codes.
- Audit rig's Copilot OAuth implementation for tracing spans or log emissions that contain the verification URI or user code. Add defense-in-depth redaction patterns for the verification URI format (`https://github.com/login/device`) and the 8-character hyphenated user code pattern.
- Xiaomi MiMo runtime failures flow through the existing retry and sanitization layers like the other keyed providers.

### Provider trust boundaries

- `providers.copilot.base_url` is unsupported because Scorpio must not redirect OAuth-backed inference traffic to user-specified hosts. Note: GitHub Enterprise-managed endpoint redirects in rig's `api-key.json` are GitHub-controlled and acceptable.
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
- Add coverage asserting that every Copilot client construction code path calls `.token_dir()` explicitly and never calls `from_env()`.
- Add Copilot auth-mode coverage proving: (a) `InteractiveSetup` path calls `.oauth()` and sets `on_device_code`; (b) `NonInteractiveRuntime` path performs a pre-flight token file existence check and fails with `TradingError::Config` if the files are absent; (c) `NonInteractiveRuntime` path sets a no-op `on_device_code` handler that returns an error if invoked.
- Add coverage that `step5_health_check` calls `GET /user` after device flow, validates `X-OAuth-Scopes`, and aborts if the scope is broader than allowed or absent.
- Add coverage that `step5_health_check` writes `scorpio-identity.json` with the numeric account ID only after successful scope validation.
- Add coverage that cached auth reuse calls `GET /user`, re-validates the numeric account ID against the stored record, and fails closed on mismatch.
- Add coverage that cached Copilot auth is rejected when `scorpio-identity.json` is missing, unreadable, or contains no numeric account ID.
- Add coverage that token directory ownership/permission check fails closed on a directory Scorpio does not own.
- Add Xiaomi MiMo `base_url` validation coverage for HTTPS, loopback exceptions (parsed via `url` crate host comparison against `{127.0.0.1, ::1, localhost}` allowlist), userinfo-presence rejection, and rejected insecure remote hosts.
- Add caller-mapping coverage or targeted integration tests proving `step5_health_check` uses the interactive Copilot auth mode while runtime readiness paths use the non-interactive mode.
- **Migration:** Before adding `ProviderId::Copilot`, delete or invert the nine existing rejection tests: `validate_provider_id_rejects_copilot` (client.rs), `deserialize_provider_name_rejects_copilot`, `load_from_rejects_copilot_provider_name`, `load_from_user_path_surfaces_friendly_error_when_saved_provider_is_copilot`, two env-override copilot error tests (config.rs), `load_user_config_at_preserves_stale_copilot_routing_strings` (settings.rs), `default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported` (model_selection.rs). Also remove `STALE_COPILOT_PROVIDER_MARKER` and the recovery wrapper in `config.rs`.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add agent-construction tests proving both new provider variants build correctly.

`crates/scorpio-core/src/providers/factory/discovery.rs`

- Add tests proving Copilot returns the curated static list with no network request and without constructing a client.
- Add a compile-time or type-level assertion verifying that `ProviderId::Copilot` never reaches the `list_models()` code path (since `CopilotExt`'s `ModelListing = Nothing` makes this a compile error).
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

- Add Copilot static-model menu coverage (confirming no Codex-class models in the list for slice 1).
- Add Xiaomi MiMo listing/manual fallback coverage.
- Add coverage that saved-but-unlisted Copilot models default to manual entry with prefilled text.
- Preserve saved-provider and saved-model defaulting tests across the expanded provider set.
- Add coverage that `discover_provider_models_blocking` for Copilot returns the static curated list without constructing a client or calling `Config::load_effective_runtime`.
- Add coverage that `discover_provider_models_blocking` for Xiaomi MiMo calls `Config::load_effective_providers_config_from_user_path` (not `Config::load_effective_runtime`) so first-run discovery works before `[llm]` routing exists.

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
