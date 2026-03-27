# Design: Copilot MCP-backed tool calling for phase 1 analysts

**Date:** 2026-03-27
**Status:** Draft

## Goal

Enable Scorpio Analyst to run phase 1 analyst agents with GitHub Copilot as the only provider by giving Copilot real access to Scorpio-managed analyst tools, removing ACP request cross-talk after timeouts, and keeping the security boundary limited to app-managed tools.

## Why this design is needed

The current Copilot provider was built as a reasoning-only ACP client. That no longer matches how phase 1 works or how the current Copilot ACP server behaves.

- Phase 1 analysts rely on runtime tools for Finnhub, news, OHLCV, and indicator computation.
- The current Copilot provider ignores `CompletionRequest.tools` and treats ACP `tool_call` updates as unexpected, so tool-backed analysts cannot complete successfully.
- The current provider reuses one long-lived ACP subprocess behind a mutex, so request timeouts leave stale ACP responses in the transport. Retries then read those stale responses and log `unexpected request ID` warnings before eventually timing out.
- Live probing against Copilot CLI 1.0.12 shows that ACP `tool_call` / `tool_call_update` messages are progress reports for tools the agent executes itself. They are not callbacks asking the client application to execute the tool.
- ACP documentation shows that client-supplied tools belong in `session/new.mcpServers`, not in custom replies to `tool_call` updates.
- The current Copilot initialize response does not advertise `mcpCapabilities.acp`, so app-supplied tools must use stdio MCP servers, not ACP-transport MCP.

## Chosen approach

Use a bounded pool of isolated Copilot workers. For each tool-enabled Copilot request, attach a session-scoped stdio MCP helper subprocess backed by Scorpio analyst tools. Add a Copilot-specific tool-aware agent path so tool definitions and Copilot session metadata are passed intentionally instead of relying on rig's in-process tool loop.

## Scope

This design includes:

- phase 1 analyst tool calling with Copilot
- ACP transport/schema refresh for the current protocol shape
- bounded Copilot worker pooling and invalidation
- a session-scoped Scorpio MCP helper subprocess
- serializable analyst tool bundle descriptors
- timeout, cancellation, and stale-response hardening
- regression and integration tests for the new path

This design does not include:

- researcher, risk, trader, or fund-manager tool bundles
- ACP file system or terminal client capabilities
- ACP-transport MCP (`mcpCapabilities.acp`)
- remote MCP servers or HTTP MCP transport
- long-lived Copilot sessions across requests

## Interface summary

| Unit | Responsibility | Input contract | Output contract |
|---|---|---|---|
| `AnalystToolBundle` | Provider-agnostic description of one analyst's runtime tools | analyst runtime context | local tools, tool definitions, helper session specs |
| `CopilotToolSessionMeta` | Provider-specific metadata attached to a Copilot tool-enabled request | bundle ID, helper policy, timeout metadata | serialized `additional_params` payload |
| `CopilotMcpSessionSpec` | Wire contract between parent process and MCP helper subprocess | session-scoped tool specs and shared state | helper startup configuration |
| `CopilotWorkerPool` | Per-model pool of reusable Copilot workers | checkout request | worker lease or explicit pool timeout |
| `CopilotWorker` | Owns one Copilot subprocess and ACP transport | one request at a time | clean returnable worker or tainted/discarded worker |
| `ScorpioMcpHelper` | Exposes session-scoped analyst tools over stdio MCP | `CopilotMcpSessionSpec` | MCP tool list and tool-call results |

## Lifecycle ownership

- `CopilotWorkerPool` creates, tracks, respawns, and discards workers.
- `CopilotWorkerLease` owns one checked-out worker for the duration of a request and is responsible for returning or discarding it.
- `CopilotToolAwareAgent` creates the session spec file before building the final `CompletionRequest`, writes its absolute path into `additional_params`, and deletes it after the provider call returns or aborts.
- `CopilotCompletionModel` validates that the referenced session spec file exists, launches the helper, starts the ACP session, and never mutates the file contents.
- `ScorpioMcpHelper` owns only its in-process tool state and exits when stdin closes or the parent terminates it.
- The parent process is the only owner allowed to delete the temp session spec file.

## Architecture changes

### 1. ACP schema refresh and transport correctness

`src/providers/acp.rs` must be updated to the current ACP schema used by Copilot CLI 1.0.12.

- `session/update` now arrives as `params: { sessionId, update: {...} }`, not the older flat `params.type` layout.
- The nested `update.sessionUpdate` discriminator must support at least:
  - `agent_message_chunk`
  - `agent_thought_chunk`
  - `tool_call`
  - `tool_call_update`
  - `plan`
- `session/new` must accept typed MCP server configs instead of always sending `mcpServers: []`.
- `session/cancel` must be implemented as a client notification helper.
- The transport should remain session-aware when parsing updates so logging and failure handling can name the affected session and request.

The key behavioral rule is: late, malformed, or mismatched ACP messages after cancellation are worker-fatal. They are not warnings on a reusable transport anymore.

### 2. Copilot worker pool

Replace the single shared `Arc<Mutex<CopilotClient>>` execution bottleneck with a bounded pool of isolated workers.

Each `CopilotWorker` owns:

- one Copilot CLI subprocess
- one initialized ACP transport
- one model ID
- one fixed tool-visibility policy

Each request:

- checks out one worker exclusively
- runs one ACP session on that worker
- either returns the worker cleanly to the pool or discards it

Pool behavior must be explicit:

- `checkout()` waits on a FIFO queue so earlier requests are served before later requests.
- if all workers are busy, callers wait until either a worker is returned or the checkout timeout expires.
- if a worker is discarded, the pool schedules immediate background respawn to restore target capacity.
- if all workers in a pool are tainted or startup begins failing, requests fail fast with a `TradingError::Rig` explaining that the Copilot worker pool is unavailable for that model.
- a worker is reusable only after a request ends cleanly and the helper subprocess has been confirmed terminated.

Initial sizing is intentionally simple:

- one pool per `(exe_path, model_id, tool policy)`
- fixed pool size of `4` for the initial phase 1 implementation, matching analyst fan-out

This keeps the implementation focused on correctness first. If later phases need tunable pool sizing, that can be added after the phase 1 path is stable.

### 3. Session-scoped MCP helper subprocess

Each tool-enabled Copilot request starts one stdio MCP helper subprocess using the Scorpio binary itself.

Proposed shape:

```text
scorpio-analyst mcp serve --session-spec <temp-file>
```

The provider attaches that helper in `session/new.mcpServers` using ACP's stdio MCP transport fields:

- `name`
- `command`
- `args`
- `env`

The helper is session-scoped, not shared across requests. This keeps tool exposure, shared tool context, and cleanup local to one request.

The session spec file:

- is written with restrictive permissions (`0600`)
- contains only non-secret session data
- is deleted when the request completes or aborts

Creation order is fixed:

1. `CopilotToolAwareAgent` resolves the analyst tool bundle.
2. `CopilotToolAwareAgent` writes `CopilotMcpSessionSpec` to a temp file.
3. `CopilotToolAwareAgent` builds `CompletionRequest.additional_params` referencing that absolute path.
4. `CopilotCompletionModel` validates and consumes that path during request execution.
5. `CopilotToolAwareAgent` deletes the temp file after the provider future completes, regardless of success or failure.

The session spec schema must be versioned so parent and helper can fail fast on mismatches.

Proposed top-level shape:

```json
{
  "version": 1,
  "session_id": "uuid",
  "tool_bundle_kind": "phase1_analyst",
  "tools": [ ... ],
  "shared_state": { ... },
  "policy": {
    "helper_name": "scorpio-analyst-phase1-tools"
  }
}
```

Required fields:

- `version`: integer schema version for compatibility checks
- `session_id`: request-local identifier used for logging only
- `tool_bundle_kind`: discriminator for helper reconstruction
- `tools`: array of serializable tool specs
- `shared_state`: serialized shared runtime state such as cached news or technical context seed data
- `policy.helper_name`: stable MCP server name used in Copilot tool allowlisting

Secrets are not serialized into the spec file. The helper inherits the same environment-based API keys and config-loading behavior as the main application.

### 4. Phase-1 analyst tool bundles

The current `Vec<Box<dyn ToolDyn>>` API is not enough for Copilot tool execution because the trait object loses the information needed to reconstruct the tool in another process.

Phase 1 therefore needs a serializable analyst tool bundle abstraction, for example `AnalystToolBundle` composed of `AnalystToolSpec` items.

Each bundle item must be able to produce three things:

1. a local `ToolDyn` instance for native providers
2. a model-visible `ToolDefinition`
3. a serializable MCP reconstruction spec for the helper subprocess

Initial supported bundle items are only the phase 1 analyst tools:

- Fundamental analyst
  - `GetFundamentals`
  - `GetEarnings`
- Sentiment analyst
  - exactly one of:
    - `GetCachedNews` when `cached_news` is present
    - `GetNews` when `cached_news` is absent
- News analyst
  - exactly one of:
    - `GetCachedNews` when `cached_news` is present
    - `GetNews` when `cached_news` is absent
  - `GetMarketNews`
  - `GetEconomicIndicators`
- Technical analyst
  - `GetOhlcv`
  - `CalculateAllIndicators`
  - `CalculateRsi`
  - `CalculateMacd`
  - `CalculateAtr`
  - `CalculateBollingerBands`
  - `CalculateIndicatorByName`

Cross-process shared-state rules must be explicit:

- cached news is serialized into the session spec and reconstructed inside the helper
- technical indicator tools recreate a helper-local `OhlcvToolContext` keyed by a context ID so `get_ohlcv` and indicator tools share one candle cache inside the helper process

This bundle pattern is intentionally phase-1-focused. Later phases can extend the same pattern once the analyst path is stable.

`AnalystToolBundle` should expose a small explicit interface:

- `fn local_tools(&self) -> Vec<Box<dyn ToolDyn>>`
- `fn tool_definitions(&self) -> Vec<ToolDefinition>`
- `fn to_mcp_session_spec(&self, request_id: &str) -> CopilotMcpSessionSpec`

### 5. Copilot-specific tool-aware agent path

The public `LlmAgent` API should stay stable so callers and retry wrappers do not need a broad rewrite.

For native providers:

- keep the current `rig::AgentBuilder::tools(...)` path unchanged

For Copilot tool-enabled agents:

- add a dedicated `LlmAgent` inner variant that is not backed by rig's in-process tool loop
- manually assemble `CompletionRequest` values for prompt/chat/typed-prompt operations
- include both tool definitions and Copilot session metadata needed by the provider

That request must include:

- `preamble`
- `chat_history`
- `documents`
- `output_schema`
- `tools` from the analyst tool bundle
- provider-specific `additional_params` carrying the serialized Copilot tool session metadata

`additional_params` must use one owned schema, not ad hoc JSON. The provider should deserialize it into a dedicated type:

```json
{
  "copilot_tool_session": {
    "version": 1,
    "helper_kind": "phase1_analyst_mcp",
    "tool_bundle_kind": "phase1_analyst",
    "helper_name": "scorpio-analyst-phase1-tools",
    "session_spec_path": "/absolute/path/to/spec.json",
    "checkout_timeout_ms": 5000,
    "request_timeout_ms": 30000
  }
}
```

Required fields are:

- `version`: metadata schema version
- `helper_kind`: discriminator for future helper variants
- `tool_bundle_kind`: lets the provider validate it received a supported Copilot bundle
- `helper_name`: stable MCP server name used for allowlisting
- `session_spec_path`: absolute path to the temp session spec file
- `checkout_timeout_ms`: copied from resolved runtime config for logs and diagnostics
- `request_timeout_ms`: copied from resolved runtime config for logs and diagnostics

If `additional_params` is missing, malformed, has the wrong version, or references a missing session spec path, the Copilot request fails before worker checkout.

`CopilotCompletionModel` reads that provider-specific metadata, launches the MCP helper, attaches the helper as an MCP server in `session/new`, then runs the ACP prompt turn.

One important consequence: `max_turns` from rig no longer drives tool execution for Copilot tool-enabled agents. Copilot performs the tool loop internally during a single ACP `session/prompt` turn. Retry, timeout, and typed-output validation remain request-level behaviors in Scorpio.

### 6. MCP helper server implementation

The helper subprocess should be implemented with the `rmcp` crate behind a hidden Scorpio CLI subcommand.

Responsibilities of the helper:

- advertise only the tools present in the session spec
- map MCP `call_tool` requests to the corresponding Scorpio tool implementation
- return normal MCP text results for successful tool calls
- return structured MCP tool errors for tool failures
- shut down cleanly when stdin closes or the parent terminates the session

The helper must not expose shell, filesystem, browsing, or arbitrary execution primitives. Its only external surface is the set of Scorpio analyst tools declared in the session spec.

On startup the helper must:

- parse the session spec file
- verify `version == 1`
- reject unknown `tool_bundle_kind`
- reconstruct exactly the declared tools and shared state
- advertise exactly one MCP server name matching `policy.helper_name`

### 7. Security boundary

The approved security boundary remains: app tools only.

That means:

- ACP client capabilities for filesystem and terminal stay disabled
- tools are provided only through the Scorpio MCP helper attached in `session/new`
- Copilot workers are started with a restrictive tool-visibility policy that:
  - disables built-in MCP sources not required for Scorpio
  - makes only the Scorpio helper tool namespace visible to the model
  - denies shell, write, URL, and other non-Scorpio capabilities where the CLI supports defense-in-depth flags

Startup validation rules must be explicit:

- verify the Copilot CLI version is at least `1.0.12`
- verify the configured worker launch arguments include the required tool-filtering flags
- verify a probe session with the attached helper MCP server exposes only the expected helper server name and no unexpected built-in tool namespaces
- if this validation cannot prove the restriction is active, provider preflight fails and the application does not start in Copilot tool-enabled mode

If the running Copilot CLI version cannot enforce that allowlist in ACP mode, worker startup fails fast with a compatibility error. The implementation must not silently widen the permission boundary.

The minimum supported Copilot CLI version for this implementation is the first version validated by tests and local probing, currently `1.0.12`.

## Data flow

```text
Analyst builds AnalystToolBundle
    -> build_agent_with_tools() chooses provider path
        -> native provider: local ToolDyn path
        -> Copilot: CopilotToolAwareAgent path

Copilot request starts
    -> serialize session spec to temp file
    -> checkout Copilot worker from per-model pool
    -> spawn `scorpio-analyst mcp serve --session-spec <file>`
    -> send `session/new` with stdio MCP server entry
    -> send `session/prompt`
    -> Copilot executes analyst tools through MCP helper
    -> provider receives `tool_call` / `tool_call_update` progress updates
    -> provider accumulates `agent_message_chunk` text
    -> provider receives final `session/prompt` response
    -> provider tears down helper and returns or discards worker
    -> tool-aware agent deletes temp spec after provider future resolves
    -> existing typed parsing + validation stays unchanged
```

The provider should treat tool progress updates as observability signals, not as execution callbacks. The actual tool execution path is Copilot -> MCP helper.

## Error handling and timeout model

Timeout handling must be redesigned so a failed request cannot poison the next request.

- Worker acquisition and ACP session execution are separate phases.
- If worker acquisition cannot complete in time, return a timeout that clearly says the request expired while waiting for a Copilot worker.
- If the ACP prompt turn times out:
  - send `session/cancel`
  - wait a short grace period for a clean `cancelled` or `stopReason` response
  - kill the Copilot worker and the MCP helper if the request is still active
  - discard the worker from the pool
- If the MCP helper fails to start, crash mid-turn, or returns unrecoverable MCP protocol errors, fail the request and discard the associated Copilot worker.
- If the ACP transport sees malformed nested updates, mismatched session IDs, or late responses after cancellation, treat the worker as tainted and discard it.
- Tool execution failures inside the helper should be surfaced back to Copilot as tool-call failures whenever possible. Only helper/process/protocol failures should abort the whole request immediately.
- Permission requests from Copilot in this app-tools-only mode should be treated as compatibility faults. The request is rejected/cancelled, and repeated violations invalidate the worker.

The system must never attempt to reuse a worker unless the request ended in a clean idle state.

## Testing strategy

### Unit tests

- ACP message parsing for nested `session/update` payloads
- ACP `session/cancel` notification serialization
- worker-pool checkout, return, discard, and respawn logic
- temp session spec lifecycle and cleanup
- analyst tool bundle conversion into:
  - local tools
  - tool definitions
  - MCP session spec entries

### MCP helper tests

Use an `rmcp` client in tests to validate the hidden helper subcommand and its tool exposure.

- list tools for each analyst bundle
- call representative fundamental, news, sentiment, and technical tools
- verify cached news reconstruction
- verify shared technical context across `get_ohlcv` and indicator tools

### Provider integration tests

Use a mock ACP agent process to validate:

- `session/new` includes the stdio MCP server entry
- nested `tool_call` / `tool_call_update` messages are parsed correctly
- `session/cancel` is sent on timeout
- a timed-out request discards the worker
- the next request after a timeout does not log or surface stale request IDs

### Workflow regression tests

- four analysts can run concurrently with Copilot as quick-thinking provider using mocked Copilot/MCP layers
- one timed-out analyst does not poison later Copilot requests
- typed analyst outputs still pass the existing validation layer

### Optional smoke test

Add an ignored local-development smoke test for real Copilot CLI 1.0.12+ that exercises one tool-enabled analyst end to end. This is useful for manual validation but must not be required in CI.

## Implementation touch points

Expected touch points for this design are:

- `src/providers/acp.rs`
- `src/providers/copilot.rs`
- `src/providers/factory.rs`
- `src/agents/analyst/*.rs`
- `src/main.rs`
- `src/cli/` for the hidden MCP helper subcommand
- one or more new MCP helper / tool-bundle modules
- `Cargo.toml` for `rmcp` and any required feature updates

## Recommended implementation order

1. Refresh ACP schema models and add `session/cancel` support.
2. Introduce the Copilot worker pool and worker invalidation rules.
3. Add the hidden MCP helper subcommand and `rmcp` server implementation.
4. Create the phase 1 analyst tool bundle descriptors and helper reconstruction logic.
5. Add the Copilot-specific tool-aware `LlmAgent` path in the provider factory layer.
6. Update phase 1 analysts to build tool bundles instead of only boxed `ToolDyn` values.
7. Attach the session-scoped MCP helper in `session/new` and enforce the Copilot tool allowlist policy.
8. Add regression and integration tests, then run `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt -- --check`.

## Success criteria

This design is successful when all of the following are true:

- `cargo run` with Copilot as the only provider no longer produces `unexpected request ID` warnings during normal phase 1 analyst fan-out.
- phase 1 analysts can successfully use their Scorpio tools through Copilot.
- a timeout or cancellation in one Copilot request cannot poison a later request.
- Copilot cannot access filesystem, shell, web, or other capabilities outside the approved Scorpio MCP helper tools.
- typed output validation and retry semantics remain intact for analyst callers.
