# Plan — OAuth-based `providers/copilot/` provider

**Date:** 2026-04-23
**Status:** Proposed
**Supersedes:** current ACP-subprocess Copilot provider (`providers/copilot.rs` + `providers/acp.rs`)
**Source research:**
- `docs/brainstorms/architectural_integration_copilot_financial.md`
- `docs/brainstorms/copilot_scorpio_integration.md`
- Reference codebase: [`farion1231/cc-switch`](https://github.com/farion1231/cc-switch)
- Runtime endpoint routing references:
  - [`src-tauri/src/proxy/forwarder.rs`](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/proxy/forwarder.rs)
  - [`src/lib/query/copilot.ts`](https://github.com/farion1231/cc-switch/blob/main/src/lib/query/copilot.ts)
  - [`src-tauri/src/proxy/copilot_optimizer.rs`](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/proxy/copilot_optimizer.rs)

## Objective

Replace the ACP subprocess-based Copilot provider with a direct HTTPS client under `crates/scorpio-core/src/providers/copilot/` that:

1. Authenticates via GitHub OAuth Device Flow and persists the durable GitHub token in `~/.scorpio-analyst/copilot_auth.json`.
2. Exchanges the GitHub token for a short-lived Copilot token on demand.
3. Queries `https://api.github.com/copilot_internal/user` to discover the runtime API base and falls back to `https://api.githubcopilot.com` when discovery returns no override.
4. Owns all Copilot-specific auth, chat, models, usage, and token-usage tracking logic inside `providers/copilot/`.
5. Preserves the existing `CopilotProviderClient` / `CopilotCompletionModel` downstream consumer surface so the agent layer and retry/rate-limit wiring do not need architectural changes, even though provider internals and factory wiring do.

Copilot remains an explicitly experimental capability. This refactor changes its auth/transport internals, not its support tier or stability guarantee.

## Scope

- **Single account only.** Unlike `cc-switch`, Scorpio will not support multiple GitHub/Copilot accounts in this pass, even though the persisted file format mirrors `cc-switch`'s v3 envelope.
- **Dynamic endpoint discovery.** Runtime requests default to `https://api.githubcopilot.com`, but if `/copilot_internal/user` returns `endpoints.api`, Scorpio caches and uses that API base instead.
- **Models + usage management.** Scorpio will own Copilot model discovery and usage/quota fetches inside the provider rather than treating chat completion as the only Copilot-specific behavior.

## Non-goals

- **No streaming.** Scorpio currently wraps Copilot's non-streaming path into a single-item stream — keep that.
- **No tool calls.** ACP impl explicitly warns; Copilot's chat endpoint shouldn't need them for Scorpio's use.
- **No multi-account support.** One persisted GitHub account is enough for Scorpio.
- **No GHES support.** This plan only targets `github.com`, `api.github.com`, and `api.githubcopilot.com` behavior.
- **No background refresh daemon.** Lazy refresh before requests is sufficient.

## Verified endpoint model

Official GitHub OAuth endpoints used by the device flow:

- `POST https://github.com/login/device/code`
- `POST https://github.com/login/oauth/access_token`
- `GET https://api.github.com/user`

Community-observed / undocumented Copilot endpoints used by the provider:

- `GET https://api.github.com/copilot_internal/v2/token`
- `GET https://api.github.com/copilot_internal/user`
- `POST {api_base}/chat/completions`
- `GET {api_base}/models`

`api_base` behavior:

- Default: `https://api.githubcopilot.com`
- Override: if `GET https://api.github.com/copilot_internal/user` returns `endpoints.api`, cache and use that dynamic API base for subsequent runtime requests

This matches the observed `cc-switch` pattern while keeping Scorpio scoped to a single account.

## Target module layout

Replace the single-file `crates/scorpio-core/src/providers/copilot.rs` implementation with a dedicated folder:

```
providers/copilot/
├── mod.rs          # Public API: CopilotProviderClient, CopilotCompletionModel, CopilotError
├── auth.rs         # copilot_auth.json store + device flow + persisted GitHub token
├── tokens.rs       # Copilot token exchange + in-memory cache + single-flight refresh
├── endpoints.rs    # /copilot_internal/user + endpoints.api cache + default fallback
├── http.rs         # reqwest client + chat/models/usage calls + IDE-spoofing headers
├── rig_impl.rs     # rig::CompletionModel impl + prompt/message translation
└── errors.rs       # CopilotError taxonomy
```

Delete `providers/acp.rs` and remove `pub mod acp` from `providers/mod.rs`. Delete ACP-specific tests from the old `copilot.rs`. The `CopilotError` variants for `SpawnFailed` / `Transport` / `ProtocolError` go away.

## Components

### 1. `auth.rs` — persisted auth state + device flow

`auth.rs` owns the durable Copilot authentication file:

- Path: `~/.scorpio-analyst/copilot_auth.json`
- Format: a single-account use of the `cc-switch` v3 JSON envelope so Scorpio can reuse the same auth model while intentionally managing only one account
- Write semantics: atomic write + `0o600`, mirroring the security posture of `settings.rs`

Persisted store shape:

```json
{
  "version": 3,
  "accounts": {
    "164835500": {
      "github_token": "ghu_...",
      "user": {
        "login": "BigtoMantraDev",
        "id": 164835500,
        "avatar_url": "https://avatars.githubusercontent.com/u/164835500?v=4"
      },
      "authenticated_at": 1775100905
    }
  },
  "default_account_id": "164835500"
}
```

Scorpio constraints on that format:

- exactly one entry in `accounts`
- `default_account_id` always points at that single stored account
- one durable GitHub token
- no persisted Copilot `tid_` token

**Device Flow** (invoked by an explicit Copilot step inside the CLI `setup` wizard, not during normal request execution):

- `POST https://github.com/login/device/code` with `client_id=Iv1.b507a08c87ecfe98` and `scope=read:user`
- Returns `device_code`, `user_code`, `verification_uri`, `interval`, `expires_in`
- Poll `POST https://github.com/login/oauth/access_token` at `interval` until success
- Handle normal device-flow responses explicitly:
  - `authorization_pending`
  - `slow_down`
  - `expired_token`
  - `access_denied`
- After access-token success, fetch `GET https://api.github.com/user` to capture account identity and verify the token works
- Persist the GitHub token to `copilot_auth.json`

The provider is responsible for reading and writing this file. `config.toml` remains the home of generic app/provider config, not Copilot OAuth state.

### 2. `tokens.rs` — Copilot token exchange + pre-request refresh

Exchange path:

- `GET https://api.github.com/copilot_internal/v2/token` with the persisted GitHub OAuth token
- Response includes a short-lived Copilot token (`tid_...`) and expiry timestamp

In-memory cache:

- `CopilotTokenCache { current: tokio::sync::RwLock<Option<CachedToken>>, refresh_lock: tokio::sync::Mutex<()> }`
- `CachedToken { value: SecretString, expires_at: Instant }`

Refresh behavior:

- **Fast path:** return cached `tid_` if not within 60s skew of expiry
- **Slow path:** take `refresh_lock`, double-check, exchange, update cache
- Refresh is a pre-request cache decision only
- Upstream `401` does **not** trigger invalidate/refresh/retry; it surfaces as `Unauthorized`

This preserves the user's decision that runtime 401s are treated as upstream auth failures, not implicit refresh signals.

### 3. `endpoints.rs` — dynamic API base discovery

`endpoints.rs` owns runtime API base discovery.

Discovery path:

- `GET https://api.github.com/copilot_internal/user` with the persisted GitHub OAuth token
- Parse the response for usage/quota information and optional `endpoints.api`

Behavior:

- `preflight_copilot_if_configured` calls `/copilot_internal/user` after successful auth load + token exchange and seeds the endpoint cache before the first runtime request
- If `endpoints.api` is present, cache it as the runtime API base
- If the response has no `endpoints.api`, fall back to `https://api.githubcopilot.com`
- Cache the discovered API base in memory for later `chat` and `models` calls
- Reuse the `/copilot_internal/user` response to power Copilot usage/quota reporting instead of making a separate usage-only code path

This is intentionally provider-managed behavior, not a user-configurable `base_url` override.

### 4. `http.rs` — chat, models, usage

Owns a shared `reqwest::Client` and Copilot-specific request construction.

Headers:

- `Editor-Version`
- `Editor-Plugin-Version`
- `User-Agent`
- `Copilot-Integration-Id: vscode-chat`
- per-instance-random `Vscode-Sessionid` and `Vscode-Machineid`

Responsibilities:

- `async fn chat_completion(...)`:
  - resolve current API base from `endpoints.rs`
  - obtain a valid `tid_` from `tokens.rs`
  - send `POST {api_base}/chat/completions`
  - on upstream `401`: return `Unauthorized`
  - on `429` / `5xx`: bubble up transiently-classifiable messages for existing retry logic
- `async fn fetch_models(...)`:
  - resolve current API base
  - call `GET {api_base}/models`
  - filter/return available Copilot models without hardcoded whitelist enforcement
- `async fn fetch_usage(...)`:
  - call `GET https://api.github.com/copilot_internal/user`
  - return typed usage/quota details and opportunistically refresh endpoint cache

### 5. `rig_impl.rs` — trait wiring + usage mapping

- Keep the `CopilotCompletionModel` / `CopilotProviderClient` / `CopilotRawResponse` type names and the downstream consumer surface they expose.
- Replace the existing ACP text transport with OpenAI-style message translation for the direct chat-completions API.
- Preserve the current prompt concatenation semantics by folding preamble/documents/output schema into the system message.
- Surface real token counts from response `usage` so `TokenUsageTracker` receives authoritative Copilot counts.
- `stream()` still wraps the full non-streaming response into a single-item stream.

### 6. `errors.rs`

```rust
pub enum CopilotError {
    NotAuthenticated,                     // no copilot_auth.json or unreadable auth state
    DeviceFlowFailed(String),             // device flow / polling failures
    AuthStoreIo(String),                  // copilot_auth.json read/write failures
    TokenExchangeFailed { status: u16, body: String },
    Unauthorized,                         // upstream 401; no implicit refresh/retry
    Http(reqwest::Error),
    RateLimited { retry_after: Option<Duration> },
    BadResponse(String),                  // JSON parse failures, missing fields
    Refusal,                              // preserved for safety-filter responses
}
```

Display strings must continue to match the current retry classifier where needed (`"rate limit"`, `"429"`, `"timeout"`, `"5xx"`).

## Factory, setup, and config changes

### `config.rs`

- Copilot no longer stores its durable auth state in `ProviderSettings.api_key`
- `config.toml` continues to own provider routing/model selection/RPM and other generic provider configuration
- `providers.copilot.base_url` is ignored for runtime endpoint selection because Copilot resolves its API base dynamically via `/copilot_internal/user`
- No `SCORPIO_COPILOT_API_KEY` contract is added in this design

### `providers/factory/client.rs`

- Delete `resolve_copilot_exe_path`, `validate_copilot_cli_path`, `resolve_copilot_exe_path_from`, the `SCORPIO_COPILOT_CLI_PATH` env contract, and all associated tests
- `create_provider_client_for` `ProviderId::Copilot` branch no longer uses the generic API-key path
- Replace it with provider-specific construction that loads `copilot_auth.json`, validates that Copilot auth is present, and constructs `CopilotProviderClient::new(model_id)` (or equivalent provider-owned constructor)
- `preflight_copilot_if_configured` should verify that persisted Copilot auth can be loaded, that a Copilot token exchange succeeds, and that `/copilot_internal/user` can be queried to seed the runtime endpoint cache; failure surfaces through `TradingError::Rig` with sanitized provider-specific messaging

### CLI wizard — `crates/scorpio-cli/src/cli/setup/steps.rs`

Copilot stays out of the generic API-key picker, but it should be an explicit step in the setup wizard.

Add a Copilot-specific wizard step that:

1. Starts device flow
2. Prints `user_code` + `verification_uri`
3. Polls until success/denial/expiry
4. Persists `copilot_auth.json`

This step is intentionally separate from `PartialConfig`, because Copilot is not a normal API-key provider anymore.

### Missing-auth messaging

- `ProviderId::missing_key_hint` should no longer pretend Copilot is API-key based
- Missing-auth guidance should come from Copilot-specific errors and preflight messages, e.g. “Run Copilot authentication via setup to create `~/.scorpio-analyst/copilot_auth.json`”

## What stays unchanged

- `CompletionModelHandle`, `ProviderClient::Copilot(CopilotProviderClient)`, `ModelTier`, all of `factory/retry.rs`, `factory/agent.rs`, rate limiter wiring, snapshot schema
- `ProviderId::Copilot` enum variant name and the string `"copilot"`
- All agent code (analysts, researchers, trader, risk, fund manager)
- Copilot remains provider-internal from the perspective of the wider runtime

## Test strategy

Drop the subprocess/mock-script tests. Replace with:

- **`auth.rs`**: unit tests against `wiremock` for device flow, polling states, GitHub user fetch, auth-file read/write, and atomic persistence semantics
- **`tokens.rs`**: unit tests for expired-token refresh, near-expiry pre-request refresh, and single-flight refresh under concurrency
- **`endpoints.rs`**: unit tests for `/copilot_internal/user` parsing, `endpoints.api` discovery, cache fill during preflight, and default fallback to `https://api.githubcopilot.com`
- **`http.rs`**: header assertions, `chat_completion` happy path, upstream `401 -> Unauthorized`, `429 -> transient classifier`, `models` fetch via discovered endpoint, usage fetch via `/copilot_internal/user`
- **`rig_impl.rs`**: migrate existing `build_prompt_text` tests to the new message translation path and assert that token usage is mapped from the real Copilot response
- **Factory/preflight**: replace CLI-path tests with missing-auth, invalid-auth, token-exchange, and `/copilot_internal/user` endpoint-seeding coverage specific to `copilot_auth.json`

Dependencies:

- Add `wiremock = "0.6"` to `scorpio-core` dev-dependencies
- Add `serde_urlencoded` for device-flow form bodies if the implementation benefits from it
- If JSON auth-store helpers need it, keep dependencies local to `scorpio-core`

## Phasing (suggested PR breakdown)

1. **PR 1 — provider scaffolding + auth bootstrap**: add `providers/copilot/` modules for auth/tokens/endpoints/errors/http, introduce `copilot_auth.json` handling using the v3 `accounts` / `default_account_id` envelope, and land the Copilot-specific wizard step needed to create that auth file. Keep old ACP runtime active. No cutover yet.
2. **PR 2 — runtime cutover**: switch `CopilotProviderClient` / `rig_impl` / factory wiring to the new direct HTTPS backend, add dynamic endpoint discovery, remove `providers/acp.rs`, delete ACP tests. CI must stay green.
3. **PR 3 — docs + UX polish**: refine the Copilot setup UX if needed, update `CLAUDE.md` / `README.md` / onboarding docs to describe `copilot_auth.json`, experimental support tier, and the direct GitHub/Copilot runtime.

This keeps the dangerous runtime swap separate from the initial auth-store scaffolding and the user-facing setup/docs work.

## Risks & open decisions

- **ToS / undocumented API risk**: this design depends on community-observed Copilot endpoints (`/copilot_internal/*`, `api.githubcopilot.com`, dynamic `endpoints.api`) that are not documented as stable public APIs by GitHub. The team is explicitly accepting that risk.
- **Experimental support tier**: Copilot remains experimental. Breakage from undocumented endpoints, header drift, or endpoint-discovery changes may require disabling or revising the integration.
- **Header version drift**: pin `Editor-Version` / `Editor-Plugin-Version` as constants in `http.rs`. If GitHub tightens validation, the likely fix is a constant bump.
- **Dynamic endpoint drift**: runtime behavior may differ per account/entitlement because `/copilot_internal/user` can override the API base. Tests should cover both discovery and fallback paths.
- **Single-account only**: Scorpio intentionally does not adopt `cc-switch`'s multi-account machinery in this pass.
- **Proxy/offline**: `reqwest` honors `HTTPS_PROXY`, but Copilot still depends on multiple upstream GitHub hosts.
- **Token storage**: the durable GitHub token lives in `~/.scorpio-analyst/copilot_auth.json` with `0o600` permissions. OS keychain integration is out of scope.

## Concrete files touched (final tally)

| Action | Path                                                     |
|--------|----------------------------------------------------------|
| New    | `crates/scorpio-core/src/providers/copilot/mod.rs`       |
| New    | `crates/scorpio-core/src/providers/copilot/auth.rs`      |
| New    | `crates/scorpio-core/src/providers/copilot/tokens.rs`    |
| New    | `crates/scorpio-core/src/providers/copilot/endpoints.rs` |
| New    | `crates/scorpio-core/src/providers/copilot/http.rs`      |
| New    | `crates/scorpio-core/src/providers/copilot/rig_impl.rs`  |
| New    | `crates/scorpio-core/src/providers/copilot/errors.rs`    |
| Delete | `crates/scorpio-core/src/providers/acp.rs`               |
| Delete | `crates/scorpio-core/src/providers/copilot.rs`           |
| Modify | `crates/scorpio-core/src/providers/mod.rs`               |
| Modify | `crates/scorpio-core/src/providers/factory/client.rs`    |
| Modify | `crates/scorpio-cli/src/cli/setup/steps.rs`              |
| Modify | `crates/scorpio-cli/src/cli/setup/mod.rs`                |
| Modify | `Cargo.toml` + `crates/scorpio-core/Cargo.toml`          |
| Modify | `CLAUDE.md`                                              |
| Modify | `README.md`                                              |
