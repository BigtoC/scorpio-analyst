# Design — upgrade `graph-flow` and `rig-core`, add DeepSeek provider

**Date:** 2026-04-28
**Author:** brainstorming session with BigtoC
**Status:** Draft — pending written-spec review and implementation plan

## Summary

Upgrade the workspace from `graph-flow 0.5.0` to `0.5.1` and from `rig-core 0.32.0` to `0.35.0`, then add DeepSeek as a first-class LLM provider across runtime config, provider construction, rate limiting, setup, and public documentation.

The key architectural decision is to integrate DeepSeek through `rig-core 0.35.0`'s native `rig::providers::deepseek` client rather than through an OpenAI-compatible alias layer. The other key design constraint is to absorb the small `rig-core` chat API break inside Scorpio's existing provider factory wrapper so agent call sites do not gain provider- or version-specific branching.

## Goals

- Upgrade `graph-flow` to `0.5.1` across the workspace.
- Upgrade `rig-core` to `0.35.0` across the workspace.
- Add DeepSeek as a first-class provider for both quick-thinking and deep-thinking tiers.
- Support DeepSeek through the same config/runtime seams as other keyed HTTP providers:
  - provider-name validation
  - per-provider settings in `[providers.<name>]`
  - API key loading
  - optional `base_url`
  - per-provider RPM limiting
- Surface DeepSeek in the interactive setup wizard.
- Surface DeepSeek in public-facing setup docs, including `README.md` and `.env.example`.
- Keep the implementation aggressive about cleanup where it reduces long-term maintenance cost.

## Non-goals

- No compatibility alias that routes DeepSeek through the OpenAI provider ID or OpenAI config section.
- No DeepSeek model defaults in the setup wizard; users enter model IDs manually.
- No broader provider-layer redesign beyond what the dependency upgrade and native DeepSeek support require.
- No speculative graph/pipeline refactor tied only to the `graph-flow` patch bump.
- No new fallback behavior for DeepSeek unless required by compiler errors or failing tests.

## Design choices

| Decision                   | Choice                                                                | Rationale                                                                                |
|----------------------------|-----------------------------------------------------------------------|------------------------------------------------------------------------------------------|
| DeepSeek integration       | First-class `ProviderId::DeepSeek` using `rig::providers::deepseek`   | Matches upstream `rig-core 0.35.0`, keeps provider identity explicit, avoids alias drift |
| Tier support               | Both quick-thinking and deep-thinking                                 | Approved user scope                                                                      |
| DeepSeek endpoint behavior | Native provider with optional `base_url` override                     | Consistent with existing HTTP provider settings                                          |
| Wizard UX                  | Include DeepSeek in provider/key/routing selection, no model defaults | Approved user scope; avoids hidden provider preference changes                           |
| Upgrade posture            | Aggressive cleanup                                                    | Remove unnecessary compatibility layers instead of preserving them                       |
| `rig-core` chat migration  | Localize in `providers/factory/agent.rs`                              | Keeps agent modules insulated from upstream API churn                                    |
| `graph-flow` upgrade shape | Straight patch upgrade unless compile/runtime evidence says otherwise | `0.5.1` appears patch-level; avoid invented refactors                                    |
| Public docs                | Update README and `.env.example`                                      | Approved user scope                                                                      |

## Architecture

The provider layer in `crates/scorpio-core/src/providers/` remains the sole integration seam for the dependency bump and the new provider.

The target architecture is:

1. Workspace dependencies move to `rig-core = 0.35.0` and `graph-flow = 0.5.1`.
2. `scorpio-core` adds `ProviderId::DeepSeek` and a matching `ProviderClient::DeepSeek` enum variant.
3. Effective runtime config accepts `deepseek` anywhere a provider name is selected for `quick_thinking_provider` or `deep_thinking_provider`.
4. `ProvidersConfig` gains a `deepseek` section with the same `ProviderSettings` shape used by the other keyed HTTP providers.
5. `Config::load_*` and the persisted setup-file boundary gain `deepseek_api_key` and the `SCORPIO_DEEPSEEK_API_KEY` env contract.
6. `create_completion_model(...)` resolves `deepseek` into a native `rig` DeepSeek client, applying optional `base_url` and rate limiter wiring.
7. `build_agent(...)`, `chat(...)`, and `chat_details(...)` continue to present Scorpio's stable internal LLM interface while absorbing the `rig-core 0.35.0` chat-history signature change internally.
8. The CLI setup wizard, README, and `.env.example` expose DeepSeek as a supported provider.

This keeps the change concentrated in the same seams that already own OpenAI, Anthropic, Gemini, Copilot, and OpenRouter.

## Components

### Workspace dependencies

`Cargo.toml`

- Update workspace dependency pins:
  - `rig-core = "0.35.0"`
  - `graph-flow = { version = "0.5.1", features = ["rig"] }`
- Do not introduce parallel crate versions or crate-local overrides.

### Provider identity and validation

`crates/scorpio-core/src/providers/mod.rs`

- Add `ProviderId::DeepSeek`.
- Extend `ProviderId::as_str()` with `"deepseek"`.
- Extend `ProviderId::missing_key_hint()` with `"SCORPIO_DEEPSEEK_API_KEY"`.
- Update tests that validate provider display and tier-to-provider mapping.

`crates/scorpio-core/src/config.rs`

- Accept `"deepseek"` in `deserialize_provider_name()`.
- Update all supported-provider error messages to include `deepseek`.
- Extend `ProvidersConfig` with `deepseek: ProviderSettings`.
- Add a default DeepSeek provider-settings constructor with the same shape as the other keyed providers.
- Extend `ProvidersConfig::settings_for(...)`, `base_url_for(...)`, and `rpm_for(...)` to cover DeepSeek.
- Load `SCORPIO_DEEPSEEK_API_KEY` into the effective runtime config using the same precedence pattern as the other LLM provider keys.

### Persisted setup boundary

`crates/scorpio-core/src/settings.rs`

- Add `deepseek_api_key: Option<String>` to `PartialConfig`.
- Include the field in the redacted `Debug` implementation.
- Extend round-trip/load/save tests to cover the new field.

### Provider construction

`crates/scorpio-core/src/providers/factory/client.rs`

- Import `rig::providers::deepseek`.
- Extend `ProviderClient` with `DeepSeek(deepseek::Client)`.
- Extend `validate_provider_id(...)` so `deepseek` resolves to `ProviderId::DeepSeek`.
- Add a DeepSeek client-construction branch to `create_provider_client_for(...)`:
  - require a configured API key
  - use `deepseek::Client::builder().api_key(...).base_url(...).build()` when `base_url` is present
  - otherwise use `deepseek::Client::new(...)`
- Extend client-construction tests to cover successful DeepSeek creation, missing-key failure, and optional base-url override behavior.

This file remains the only place where provider-name strings become concrete provider clients.

### Agent construction and chat API migration

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add DeepSeek to the internal `LlmAgentInner`/enum-dispatch path alongside the existing provider variants.
- Extend `build_agent_inner(...)` to construct a DeepSeek-backed agent through the same `CompletionClient` pattern used elsewhere.
- Absorb the `rig-core 0.35.0` chat API change inside Scorpio's wrapper methods rather than pushing it outward.

The relevant upstream change is small but real:

- In `rig-core 0.32.0`, `Agent::chat(...)` took a concrete `Vec<Message>` path and Scorpio mirrored that assumption.
- In `rig-core 0.35.0`, the chat path is generalized around iterable history input and the `PromptRequest::with_history(...)` flow.

Scorpio's design response is:

- keep `LlmAgent::chat(prompt, chat_history: Vec<Message>) -> Result<String, PromptError>` as the stable wrapper for copy-on-call semantics
- keep `LlmAgent::chat_details(prompt, chat_history: &mut Vec<Message>) -> Result<PromptResponse, PromptError>` as the stable wrapper for in-place mutable-history semantics
- implement both wrappers in terms of the current `rig-core 0.35.0` request API

That preserves debate/risk/researcher/trader call sites and localizes version churn to one file.

### Rate limiting

`crates/scorpio-core/src/rate_limit.rs`

- Add DeepSeek to `ProviderRateLimiters::from_config(...)`.
- Extend test helpers that map `ProviderId` to mutable RPM fields.
- Ensure DeepSeek is treated like the other keyed HTTP providers: `rpm == 0` disables the limiter.

### Setup wizard

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Add DeepSeek to `WIZARD_PROVIDERS`.
- Extend `validate_step3_result(...)`, `provider_key(...)`, `set_provider_key(...)`, and any related helper logic to include `deepseek_api_key`.
- Allow DeepSeek to participate in routing eligibility for both model tiers.
- Keep model prompts free-form with no DeepSeek defaults or recommendations.

### Public docs

`README.md`

- Update setup guidance, provider examples, and provider lists to include DeepSeek wherever the project enumerates supported LLM providers.
- Keep the wording factual rather than marketing-heavy.

`.env.example`

- Add `SCORPIO_DEEPSEEK_API_KEY=` alongside the other LLM provider keys.

## Runtime flow

1. The user configures DeepSeek through `scorpio setup`, direct config editing, `.env`, or `SCORPIO__LLM__...` routing overrides.
2. `Config::load_*` merges user config, `.env`, nested `SCORPIO__...` settings, and flat API-key env vars into the effective runtime config.
3. `create_completion_model(...)` reads the provider name for the selected tier.
4. When that provider is `deepseek`, the factory:
   - resolves `providers.deepseek`
   - requires an API key
   - applies optional `base_url`
   - attaches the DeepSeek provider rate limiter
   - returns a `CompletionModelHandle` whose concrete client is DeepSeek-backed
5. `build_agent(...)` wraps the concrete DeepSeek client in the provider-agnostic `LlmAgent` interface.
6. Upstream callers continue to use `prompt_with_retry(...)`, `prompt_with_retry_details(...)`, `chat_with_retry(...)`, and `chat_with_retry_details(...)` without provider-specific branching.

This keeps DeepSeek as a normal tier-selectable provider rather than a special-case code path.

## Error handling

The change preserves the existing failure model.

### Config-time failures

- Unknown provider names continue to fail during config deserialization, now listing `deepseek` among supported values.
- Missing DeepSeek credentials fail as `TradingError::Config` with the hint `SCORPIO_DEEPSEEK_API_KEY`.
- Invalid `providers.deepseek.base_url` values fail when constructing the DeepSeek client, matching the existing provider-client builder behavior.

### Runtime/provider failures

- DeepSeek request failures flow through the existing provider retry and error-sanitization helpers.
- Error summaries should continue to use the standard `provider=<name> model=<id> summary=...` shape so logs and tests stay consistent.
- No DeepSeek-specific retry branch should be added unless real failing tests demonstrate a provider-specific need.

### Dependency-upgrade failures

- `graph-flow 0.5.1` is expected to be a patch-level dependency lift.
- If the upgrade surfaces a compile- or behavior-level delta, adapt only the directly impacted graph-flow call sites and preserve Scorpio's existing error mapping into `TradingError::GraphFlow`.

## Testing

### Unit and module coverage

`crates/scorpio-core/src/config.rs`

- Add provider-name validation coverage for `deepseek`.
- Extend unknown-provider error-message assertions to list `deepseek`.
- Add config-loading tests for `SCORPIO_DEEPSEEK_API_KEY` if they do not already exist through shared helpers.

`crates/scorpio-core/src/settings.rs`

- Add round-trip coverage for `deepseek_api_key`.
- Extend debug-redaction assertions so the new secret never appears in plain text.

`crates/scorpio-core/src/providers/factory/client.rs`

- Add factory-construction tests for DeepSeek success, missing-key failure, validation, and base-url override behavior.

`crates/scorpio-core/src/providers/factory/agent.rs`

- Add or update tests proving that DeepSeek agents are built correctly.
- Add or update tests around `LlmAgent::chat` and `chat_details` so the `rig-core 0.35.0` chat migration preserves:
  - copy-on-call history behavior for `chat(...)`
  - in-place mutable-history behavior for `chat_details(...)`
  - existing retry-wrapper expectations

`crates/scorpio-core/src/rate_limit.rs`

- Extend provider limiter construction tests to include DeepSeek.

`crates/scorpio-cli/src/cli/setup/steps.rs`

- Extend wizard tests for provider lists, secret persistence, eligibility filtering, and routing selection to include DeepSeek.

### Regression coverage

- Keep existing pipeline/runtime/provider regression tests as the proof that the dependency upgrades do not disturb graph construction, task orchestration, or normal model invocation behavior.
- Do not reduce existing provider coverage while adding DeepSeek.

### Verification commands

Implementation is only complete after all required repo checks pass in order:

1. `cargo fmt -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo nextest run --workspace --all-features --locked --no-fail-fast`

## Implementation boundaries

The resulting implementation plan should stay inside these boundaries:

- keep changes concentrated in the provider/config/setup/docs seams already listed above
- prefer extending existing helper tables and enums over introducing new abstraction layers
- avoid creating an OpenAI-compat DeepSeek wrapper that obscures provider identity
- avoid graph-flow refactors not justified by an actual `0.5.1` integration need
- preserve Scorpio's internal LLM wrapper API even if the upstream `rig-core` chat shape changed

## Approved direction

This design reflects the validated brainstorming decisions:

- DeepSeek is first-class, not an alias.
- DeepSeek supports both thinking tiers.
- DeepSeek supports optional `base_url` overrides.
- The wizard includes DeepSeek, but with manual model entry and no defaults.
- README and `.env.example` both surface DeepSeek publicly.
- Cleanup is preferred where it removes needless compatibility code.
