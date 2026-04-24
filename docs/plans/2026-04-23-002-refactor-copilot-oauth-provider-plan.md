# Plan — OAuth-based `copilot.rs` provider

**Date:** 2026-04-23
**Status:** Proposed
**Supersedes:** current ACP-subprocess Copilot provider (`providers/copilot.rs` + `providers/acp.rs`)
**Source research:**
- `docs/brainstorms/architectural_integration_copilot_financial.md`
- `docs/brainstorms/copilot_scorpio_integration.md`
- Reference codebase: [`farion1231/cc-switch`](https://github.com/farion1231/cc-switch)

## Objective

Replace the ACP subprocess-based Copilot provider with a direct HTTPS client that:

1. Authenticates via GitHub OAuth Device Flow to obtain a durable `ghu_` token.
2. Exchanges it for a short-lived `tid_` Copilot token on demand.
3. Calls `api.githubcopilot.com/chat/completions` with IDE-spoofing headers.
4. Preserves the existing `CopilotProviderClient` / `CopilotCompletionModel` public surface so downstream consumers like the agent layer and retry/rate-limit wiring do not need to change, even though provider factory wiring still does.

Copilot remains an explicitly experimental capability. This refactor changes its auth/transport internals, not its support tier or stability guarantee.

## Non-goals

- **No streaming.** Scorpio currently wraps Copilot's non-streaming path into a single-item stream — keep that.
- **No tool calls.** ACP impl explicitly warns; Copilot's chat endpoint shouldn't need them for scorpio's use.
- **No background refresh daemon.** Lazy refresh on expiry / 401 is sufficient.

## Target module layout

New submodule under `crates/scorpio-core/src/providers/copilot/` replacing the single `copilot.rs`:

```
providers/copilot/
├── mod.rs          # Public API: CopilotProviderClient, CopilotCompletionModel, CopilotError
├── oauth.rs        # Device flow + token cache + refresh (single-flight)
├── http.rs         # reqwest client + IDE-spoofing header injection + OpenAI payload mapping
├── rig_impl.rs     # rig::CompletionModel impl (build_prompt_text lives here, lifted from current copilot.rs)
└── errors.rs       # CopilotError taxonomy
```

Delete `providers/acp.rs` and remove `pub mod acp` from `providers/mod.rs`. Delete ACP-specific tests from the old `copilot.rs`. The `CopilotError` variants for `SpawnFailed` / `Transport` / `ProtocolError` go away.

## Components

### 1. `oauth.rs` — token lifecycle

**Device Flow** (invoked by the CLI `setup` wizard, not at runtime):

- `POST https://github.com/login/device/code` with `client_id=Iv1.b507a08c87ecfe98` (the well-known Copilot CLI client ID used by cc-switch) and `scope=read:user`.
- Returns `device_code`, `user_code`, `verification_uri`, `interval`.
- Poll `POST https://github.com/login/oauth/access_token` at `interval` until success → `ghu_...` token.

**Token exchange:**

- `GET https://api.github.com/copilot_internal/v2/token` with `Authorization: Bearer <ghu_>` → JSON with `token` (starts `tid_`) + `expires_at` (Unix ts).

**Cache + refresh:**

- `CopilotTokenCache { current: tokio::sync::RwLock<Option<CachedToken>>, refresh_lock: tokio::sync::Mutex<()> }`.
- `CachedToken { value: SecretString, expires_at: Instant }`.
- `async fn access_token(&self) -> Result<SecretString>`:
  - **Fast path:** read lock, return if `expires_at > now + 60s skew`.
  - **Slow path:** acquire `refresh_lock` (single-flight), double-check, exchange, write lock. This prevents a thundering herd on expiry when all agents race.
- Refresh is a pre-request cache decision only: if the cached token is expired or within skew, exchange before sending the request.
- Upstream `401` from the chat endpoint does **not** trigger invalidate/refresh/retry; it surfaces as `Unauthorized`.

### 2. `http.rs` — chat client

- Owns a `reqwest::Client` (built once; reuse connection pool).
- `fn ide_headers() -> HeaderMap` — static `Editor-Version`, `Editor-Plugin-Version`, `User-Agent`, `Copilot-Integration-Id: vscode-chat`; per-instance-random `Vscode-Sessionid` and `Vscode-Machineid` (UUID v4, generated once and stored on the client).
- `async fn chat_completion(&self, req: OpenAiChatRequest) -> Result<OpenAiChatResponse>`:
  - Before sending, obtains a valid `Authorization: Bearer <tid_>` from the cache/token manager; if the cached token is expired or within skew, refresh first.
  - On upstream `401`: return `Unauthorized` without invalidating the cache or retrying.
  - On `429` / `5xx`: bubble up as an error string that `is_transient_message` already classifies — the retry layer in `factory/retry.rs` takes over (no new retry logic in the provider).
- Endpoint: `https://api.githubcopilot.com/chat/completions`.

### 3. `rig_impl.rs` — trait wiring

- Keep the `CopilotCompletionModel` / `CopilotProviderClient` / `CopilotRawResponse` type names and the downstream consumer surface they expose.
- The existing text-format `build_prompt_text` is replaced with OpenAI-format message translation: `CompletionRequest` → `Vec<{role, content}>`. Preamble becomes a `system` message; documents and `output_schema` are folded into the system message (same concatenation strategy as today, but as a system message instead of a `[System]`/`[Documents]` tagged text blob). This preserves scorpio's typed-prompt + schema behavior unchanged from the agent's perspective.
- `CopilotRawResponse` now surfaces **real** token counts from the response `usage` field — a net win over ACP's all-zero sentinel. `GetTokenUsage` returns `Some(Usage)` instead of `None`. `TokenUsageTracker` will start getting authoritative Copilot counts.
- `stream()` still collects the full response and wraps it as a single-item stream (Copilot's chat endpoint does support SSE, but scorpio doesn't consume streaming anywhere — skip the complexity).

### 4. `errors.rs`

```rust
pub enum CopilotError {
    NotAuthenticated,                     // no ghu_ token in config
    DeviceFlowFailed(String),             // device flow RPC errors
    TokenExchangeFailed { status: u16, body: String },
    Unauthorized,                         // upstream 401; no implicit refresh/retry
    Http(reqwest::Error),
    RateLimited { retry_after: Option<Duration> },
    BadResponse(String),                  // JSON parse failures, missing fields
    Refusal,                              // preserved for safety-filter responses
}
```

Display strings must include phrases that match `is_transient_message` in `factory/retry.rs:547` (`"rate limit"`, `"429"`, `"timeout"`, `"5xx"`) so the existing retry classifier fires without changes.

## Factory & config changes

### `config.rs`

- `ProviderSettings.api_key` for `copilot` now holds the `ghu_` OAuth token (keep `SecretString`, keep `0o600` write, keep env override). No schema change.
- Add env var `SCORPIO_COPILOT_API_KEY` following the existing `SCORPIO_*_API_KEY` convention — keeps `missing_key_hint` uniform.
- Update `ProviderId::missing_key_hint` for `Copilot` to return `"SCORPIO_COPILOT_API_KEY"` (no longer `"(no API key required...)"`).

### `providers/factory/client.rs`

- Delete `resolve_copilot_exe_path`, `validate_copilot_cli_path`, `resolve_copilot_exe_path_from`, the `SCORPIO_COPILOT_CLI_PATH` env contract, and all associated tests (lines 280–357 + the `CopilotCliPathEnvGuard` test scaffolding ~l.403–456, ~l.843–1029).
- `create_provider_client_for` `ProviderId::Copilot` branch becomes symmetric with OpenAI/Anthropic for API-key presence only: extract `api_key` → `missing_key_error` if absent → construct `CopilotProviderClient::new(token, model_id)`.
- Copilot does not support `base_url` override. Its GitHub/Copilot endpoints are hardcoded because the provider spans multiple hosts.
- `preflight_copilot_if_configured` now calls a cheap `GET /copilot_internal/v2/token` instead of spawning a process. Same error-mapping to `TradingError::Rig` via `sanitize_error_summary`.

### CLI wizard — `crates/scorpio-cli/src/cli/setup/steps.rs`

Replace any "install Copilot CLI + set `SCORPIO_COPILOT_CLI_PATH`" step with a Device Flow step:

1. Hit `/login/device/code`, print `user_code` + `verification_uri`, wait for user Enter.
2. Poll `/login/oauth/access_token` until token arrives (timeout ~5 min).
3. Stash `ghu_...` in `PartialConfig.copilot.api_key`.

Keep the wizard non-interactive-friendly: if env var `SCORPIO_COPILOT_API_KEY` is set, skip the flow.

## What stays unchanged

- `CompletionModelHandle`, `ProviderClient::Copilot(CopilotProviderClient)`, `ModelTier`, all of `factory/retry.rs`, `factory/agent.rs`, rate limiter wiring, snapshot schema. This is a provider-internal swap.
- `ProviderId::Copilot` enum variant name and the string `"copilot"`.
- All agent code (analysts, researchers, trader, risk, fund manager).
- `TokenUsageTracker` — it will start receiving non-zero counts, which is backward-compatible.

## Test strategy

Drop the subprocess/mock-script tests (they test the wrong thing now). Replace with:

- **`oauth.rs`**: unit tests against `wiremock` mocking device flow + exchange endpoints. Cover: expired-token refresh, near-expiry pre-request refresh, single-flight refresh (two concurrent `access_token()` calls → one exchange).
- **`http.rs`**: `wiremock` asserts the spoofing headers are sent exactly (regression guard — if we ever accidentally drop `Vscode-Sessionid`, tests fail). Cover: 200 path, upstream 401 → `Unauthorized` without invalidate/retry, 429 → error-string-contains-"rate limit" so the retry classifier matches.
- **`rig_impl.rs`**: the existing `build_prompt_text` tests migrate to the new message-translation function. Assert that the system-role message contains preamble + documents + schema (order preserved).
- **Factory**: replace `CopilotCliPathEnvGuard` tests with API-key presence/absence tests identical to OpenAI's.

Add `wiremock = "0.6"` to `[dev-dependencies]` and `serde_urlencoded` (for device-flow form bodies) to `[dependencies]` of `scorpio-core`, via workspace entries.

## Phasing (suggested PR breakdown)

1. **PR 1 — scaffolding**: introduce `providers/copilot/` submodule, add `oauth.rs` + `http.rs` + `errors.rs`, keep old `copilot.rs` functioning. No factory wiring change yet. All tests for new code pass in isolation.
2. **PR 2 — cutover**: switch `rig_impl.rs` to the new backend, rewrite `factory/client.rs` Copilot branch, delete `providers/acp.rs`, delete ACP tests. CI must stay green. This is the risky one — bisectable if anything regresses.
3. **PR 3 — CLI wizard + docs**: replace the `SCORPIO_COPILOT_CLI_PATH` setup step with a device-flow step, update `CLAUDE.md` (remove "ACP over JSON-RPC" language → "OAuth device flow + api.githubcopilot.com direct"), update `missing_key_hint`.

## Risks & open decisions

- **ToS**: `architectural_integration_copilot_financial.md` §4.1 acknowledges this violates GitHub's ToS. The team is accepting that risk — a one-line `tracing::warn!` at startup ("Copilot provider uses undocumented endpoints...") makes it auditable in logs, but no further gating.
- **Experimental support tier**: Copilot remains an explicitly experimental capability. This refactor does not upgrade its support promise; breakage from undocumented endpoints or header drift may require disabling or revising the integration.
- **Header version drift**: pin `Editor-Version` / `Editor-Plugin-Version` as constants in `http.rs` with a `// TODO: bump if Copilot starts rejecting these — cross-reference cc-switch latest release`. If GitHub tightens validation, the fix is a constant bump.
- **Model availability**: don't hardcode a model whitelist — pass whatever model ID is configured. If Copilot returns 400, the error surfaces normally. (The gap-analysis doc's advice to restrict to `gpt-4`/`gpt-4-turbo`/`gpt-3.5-turbo` is outdated — Copilot now routes Claude, GPT-5-mini, o3-mini, etc.)
- **No base URL override**: `providers.copilot.base_url` is unsupported/ignored. Copilot uses fixed GitHub/Copilot endpoints to avoid ambiguous multi-host override semantics.
- **Proxy/offline**: `reqwest` honors `HTTPS_PROXY`. No additional config needed.
- **Token storage**: `ghu_` token lives in `~/.scorpio-analyst/config.toml` (0o600) just like every other provider key. OS keychain integration is out of scope.

## Concrete files touched (final tally)

| Action | Path                                                                                                      |
|--------|-----------------------------------------------------------------------------------------------------------|
| New    | `crates/scorpio-core/src/providers/copilot/mod.rs`                                                        |
| New    | `crates/scorpio-core/src/providers/copilot/oauth.rs`                                                      |
| New    | `crates/scorpio-core/src/providers/copilot/http.rs`                                                       |
| New    | `crates/scorpio-core/src/providers/copilot/rig_impl.rs`                                                   |
| New    | `crates/scorpio-core/src/providers/copilot/errors.rs`                                                     |
| Delete | `crates/scorpio-core/src/providers/acp.rs`                                                                |
| Delete | `crates/scorpio-core/src/providers/copilot.rs` (old single-file version)                                  |
| Modify | `crates/scorpio-core/src/providers/mod.rs` (drop `pub mod acp`)                                           |
| Modify | `crates/scorpio-core/src/providers/factory/client.rs` (rewrite Copilot branch, remove CLI-path machinery) |
| Modify | `crates/scorpio-core/src/providers/mod.rs::ProviderId::missing_key_hint`                                  |
| Modify | `crates/scorpio-cli/src/cli/setup/steps.rs` (device-flow wizard step)                                     |
| Modify | `Cargo.toml` (workspace) + `crates/scorpio-core/Cargo.toml` (add `wiremock` dev-dep, `serde_urlencoded`)  |
| Modify | `CLAUDE.md` (update Copilot architecture description)                                                     |
