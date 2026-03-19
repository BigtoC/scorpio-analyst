# Design for `add-copilot-provider`

## Context

The `add-llm-providers` change established a provider factory supporting OpenAI, Anthropic, and Gemini through
`rig-core`'s native provider integrations. GitHub Copilot cannot be integrated the same way because it lacks a public
REST API. Instead, GitHub provides the Copilot CLI with an ACP (Agent Client Protocol) server mode that communicates
over stdio using NDJSON-encoded JSON-RPC 2.0 messages. This design captures how the Copilot provider bridges the ACP
protocol into `rig`'s `CompletionModel` trait, enabling transparent use of Copilot as a drop-in LLM backend.

**Stakeholders:** Agent layer (all agents may route to Copilot), provider factory (registration), configuration
(provider name resolution).

## Goals / Non-Goals

- **Goals:**
  - Spawn and manage the Copilot CLI subprocess (`copilot --acp --stdio`) as an ACP server.
  - Validate at application startup that ACP connectivity can be established whenever Copilot is configured for any active LLM tier.
  - Implement the ACP client-side protocol lifecycle: `initialize` -> `session/new` -> `session/prompt` -> cleanup.
  - Parse NDJSON-framed JSON-RPC 2.0 messages from the Copilot process stdout.
  - Implement `rig`'s `ProviderClient`, `CompletionClient`, and `CompletionModel` traits so the Copilot provider is
    interchangeable with OpenAI/Anthropic/Gemini inside the existing provider factory and agent builder patterns.
  - Register `"copilot"` as a valid provider name in the existing provider factory.
  - Map ACP errors and subprocess failures into the established `TradingError` hierarchy.
  - Handle graceful shutdown of the Copilot subprocess.
  - Preserve compatibility with shared `prompt` and `chat` invocation helpers from `add-llm-providers`.

- **Non-Goals:**
  - Implementing ACP client capabilities (`fs/read_text_file`, `fs/write_text_file`, `terminal/*`) — the Copilot
    provider acts as a minimal ACP client that does not grant file system or terminal access to the Copilot agent.
  - Supporting `session/load` (session resumption) — each `rig` completion call creates a fresh ACP session.
  - Implementing MCP server connections — no MCP servers are passed to `session/new`.
  - Supporting image, audio, or embedded context content blocks — only text prompts are sent.
  - Implementing the `session/request_permission` handler beyond refusing all permission requests (Copilot should not
    execute tools on our behalf; we are using it purely as a reasoning engine).
  - Streaming partial responses to callers — the provider collects the full response before returning to `rig`.
  - Computing heuristic token estimates from visible prompt/response text for Copilot-backed calls in the MVP — ACP
    does not expose authoritative token usage, so any estimate would be approximate-only and is deferred until the
    post-MVP phase if it proves valuable.

## Architectural Overview

```
                      ┌───────────────────────────────────────────┐
                      │          src/providers/                   │
                      │                                           │
                      │  factory.rs                               │
                      │    └─ "copilot" ──► CopilotCompletionModel│
                      │                                           │
                      │  copilot.rs                               │
                      │    ├─ CopilotClient (process lifecycle)   │
                      │    └─ CopilotCompletionModel              │
                      │         (impl CompletionModel)            │
                      │                                           │
                      │  acp.rs                                   │
                      │    ├─ AcpTransport (NDJSON read/write)    │
                      │    ├─ JSON-RPC 2.0 message types          │
                      │    └─ ACP protocol methods                │
                      └───────────────────────────────────────────┘
                                 │ spawns
                                 ▼
                      ┌─────────────────────────┐
                      │  copilot --acp --stdio  │
                      │  (child process)        │
                      │                         │
                      │  stdin  ◄── NDJSON req  │
                      │  stdout ──► NDJSON resp │
                      │  stderr ──► diagnostics │
                      └─────────────────────────┘
```

### Module Responsibilities

**`acp.rs` — ACP Transport Layer:**
- Owns the low-level NDJSON read/write logic over `tokio::process::ChildStdin` / `ChildStdout`.
- Defines JSON-RPC 2.0 request/response/notification types (`JsonRpcRequest`, `JsonRpcResponse`,
  `JsonRpcNotification`) as `serde`-serializable structs.
- Provides typed ACP method wrappers: `send_initialize`, `send_session_new`, `send_session_prompt`,
  `handle_session_update` (notification processing).
- Manages JSON-RPC request ID sequencing.
- Handles NDJSON framing: each JSON message is a single line terminated by `\n`.

**`copilot.rs` — Copilot Provider:**
- `CopilotClient`: manages the lifecycle of the Copilot CLI subprocess. When Copilot is configured for any active tier,
  application startup performs a connectivity preflight by spawning the process, sending `initialize`, and retaining the
  verified `AcpTransport` handle for later reuse. If the process dies later, the client can respawn it on demand.
- `CopilotProviderClient` / `CopilotCompletionClient`: thin wrappers that satisfy the `rig` provider trait boundaries
  expected by the provider layer while delegating transport work to `CopilotClient`.
- `CopilotCompletionModel`: implements `rig::completion::CompletionModel`. On each `completion()` or history-aware chat
  call:
  1. Ensures the `CopilotClient` is initialized and still alive (normally satisfied by startup preflight; otherwise
     respawn + ACP initialize before proceeding).
  2. Sends `session/new` to create a fresh session.
  3. Translates the `rig` completion request (system prompt + user message, plus prior message history when present)
     into an ACP `session/prompt` call.
  4. Processes `session/update` notifications, accumulating `agent_message_chunk` text content.
  5. Waits for the `session/prompt` response (with `stopReason`).
  6. Assembles the accumulated text into a `rig` `CompletionResponse`.
  7. Does NOT terminate the subprocess between calls — the client persists for reuse.

### Shared Client Concurrency Model

ACP over stdio is effectively a single ordered stream. The provider therefore serializes access to the live
`CopilotClient` transport so concurrent callers cannot interleave NDJSON messages or steal each other's responses.

1. The shared client handle is protected by an async mutex at the request boundary.
2. Each completion or chat invocation holds exclusive transport access only for the duration of a single ACP session.
3. Higher-level concurrency still exists at the workflow level because non-Copilot providers operate independently, but
   Copilot-backed requests are intentionally serialized per process instance for correctness.
4. If future performance work is required, the provider may evolve to a small process pool, but MVP behavior is a single
   safe shared process.

### ACP Protocol Lifecycle Per Completion

```
  Application startup
       │
       ├── if any configured provider == "copilot"
       ├── spawn `copilot --acp --stdio`
       ├── send `initialize` { protocolVersion: 1, clientCapabilities: {} }
       ├── receive `initialize` response (verify compatibility)
       └── retain live client for reuse

  CopilotCompletionModel::completion(request)
       │
       ├── ensure live initialized client (respawn + reinitialize only if prior process died)
       ├── send `session/new` { cwd: ".", mcpServers: [] }
       ├── receive `session/new` response { sessionId }
       │
       ├── send `session/prompt` { sessionId, prompt: [{ type: "text", text: "..." }] }
       ├── loop: receive `session/update` notifications
       │     ├── agent_message_chunk → accumulate text
       │     ├── tool_call → (not expected; log warning if received)
       │     └── request_permission → respond with { outcome: "cancelled" }
       ├── receive `session/prompt` response { stopReason }
       │
       └── return CompletionResponse { text, stopReason metadata }
```

### Process Lifecycle Strategy

- **Configuration-gated startup initialization**: The Copilot subprocess is not started unless Copilot is configured for
  an active provider tier. If Copilot is configured, application startup MUST establish the ACP connection immediately by
  spawning the process and completing `initialize`, so misconfiguration or missing Copilot auth fails fast before any
  agent work begins.
- **Persistent process**: Once startup validation succeeds, the subprocess persists across multiple completion calls
  within the same application run. This avoids the overhead of repeated process spawn + ACP initialization for every
  agent call.
- **Session-per-call**: Each `completion()` call creates a new ACP session via `session/new`. This provides clean
  isolation between unrelated agent prompts without carrying over context from previous calls.
- **Graceful shutdown**: On `Drop` (or explicit shutdown), the client closes stdin to signal EOF to the Copilot process,
  then sends SIGTERM if the process doesn't exit within a timeout (2 seconds).

## Key Decisions

- **Decision: Minimal ACP client capabilities** — We advertise empty `clientCapabilities` (no `fs`, no `terminal`).
  The Copilot agent is used purely as a reasoning engine, not as a code-editing agent. This prevents Copilot from
  attempting to read/write files or execute shell commands through our application.
  - *Alternatives considered:* Providing full fs/terminal capabilities would let Copilot use tools, but introduces
    security risks and complexity far beyond the scope of a completion provider.

- **Decision: Refuse all permission requests** — If Copilot sends `session/request_permission`, we respond with
  `{ outcome: { outcome: "cancelled" } }`. This is a safety boundary.
  - *Alternatives considered:* Auto-approving permissions would require implementing tool execution, which conflicts
    with the non-goal of keeping this a pure reasoning provider.

- **Decision: Session-per-call, not persistent sessions** — Each `rig` completion call maps to a fresh ACP session.
  The `rig` `CompletionModel` trait is stateless per call; persistent sessions would require external session tracking
  that conflicts with the provider layer's design.
  - *Alternatives considered:* Reusing sessions across calls could reduce overhead, but would introduce hidden state
    coupling between unrelated agent prompts and complicate error recovery.

- **Decision: Implement the full `rig` provider trait boundary** — The PRD explicitly calls for `ProviderClient`,
  `CompletionClient`, and `CompletionModel` compatibility. The Copilot integration therefore exposes the same trait
  surface area as native providers instead of only a bespoke completion wrapper.
  - *Alternatives considered:* Implementing only `CompletionModel` would be simpler short term, but would diverge from
    the PRD and make future factory/agent composition less uniform.

- **Decision: Serialize ACP transport access** — Because stdio NDJSON traffic is ordered and session responses can be
  interleaved with notifications, a single process instance is guarded by an async mutex for correctness.
  - *Alternatives considered:* Allowing unconstrained concurrent use of one process risks response corruption; spawning a
    new process per request increases latency and operational overhead.

- **Decision: Collect full response before returning** — We accumulate all `agent_message_chunk` notifications into
  a single response string rather than streaming. The `rig` `CompletionModel` trait expects a complete response, and
  the downstream agents parse structured JSON from the response.
  - *Alternatives considered:* Streaming would reduce time-to-first-token but is incompatible with `rig`'s
    synchronous completion model and the requirement for structured JSON output parsing.

- **Decision: Configuration-gated startup initialization** — The Copilot process is only started for configurations
  that actually select `"copilot"`, but once selected it is initialized during application startup rather than waiting
  for the first completion request. This gives a fast-fail guarantee that ACP is reachable before workflows begin.
  - *Alternatives considered:* Pure lazy spawn on the first completion request saves startup work, but delays detection of
    missing CLI/authentication problems until after the user has already begun a run.

- **Decision: Factory registration via `"copilot"` provider string** — The `add-copilot-provider` change adds a
  `"copilot"` match arm to the existing provider factory. This is the only modification to `factory.rs`.
  - *Alternatives considered:* A plugin registry system would be more extensible but overengineered for a single
    additional provider.

## Risks / Trade-offs

- **Copilot CLI must be installed and authenticated** — If the `copilot` binary is not on `$PATH`, the executable path
  is wrong, or the user is not authenticated, startup validation fails before the application begins work with a clear
  `TradingError::Rig` diagnostic. Mitigation: fail fast during startup for configured Copilot tiers, while still keeping
  `config check` useful as an explicit preflight command.

- **ACP protocol is in public preview** — The protocol may change. Mitigation: the ACP transport layer (`acp.rs`)
  isolates all protocol-specific logic; breaking changes are contained to that module. We pin to protocol version 1.

- **No token usage metadata from ACP** — The ACP protocol does not expose authoritative token counts in prompt
  responses. We can only see the visible prompt/response text on the client side, so a rough estimate is technically
  possible, but it would still miss hidden system prompts, Copilot/backend prompt transformations, tokenizer/model
  differences, and any provider-side accounting not surfaced through ACP. For MVP, the `TokenUsageTracker` records
  accurate wall-clock latency and uses documented zero-value token fields as an "unavailable/not reported" sentinel for
  Copilot rather than pretending they are measured counts. Mitigation: document the limitation clearly and treat any
  future text-based estimate as heuristic-only, not audit-grade usage data.

- **Subprocess management complexity** — Managing a child process (spawn, stdio piping, graceful shutdown, crash
  recovery) adds complexity compared to HTTP-based providers. Mitigation: the `AcpTransport` struct encapsulates
  all process management; the `CopilotCompletionModel` only interacts through typed method calls.

- **Serialized Copilot requests reduce parallel throughput** — A single safe subprocess means Copilot-backed requests do
  not execute in parallel against one process instance. Mitigation: this is acceptable for MVP correctness; a future
  process pool can be introduced behind the same provider interface if throughput becomes a bottleneck.

- **Copilot response time variance** — Copilot CLI may have higher latency than direct API calls. Mitigation:
  the same `agent_timeout_secs` timeout from the provider layer applies; if Copilot exceeds the deadline, the
  request fails with `TradingError::NetworkTimeout` like any other provider.

## Open Questions

- Should the Copilot CLI executable path be configurable via `ApiConfig` (e.g., `copilot_cli_path`) or always
  resolved from `$PATH`? Recommendation: make it configurable with a default of `"copilot"`.
- Should there be a lightweight startup mode that validates ACP connectivity and then tears the subprocess down, or
  should startup keep the verified process warm for the first request? Recommendation: keep the verified process alive
  for MVP simplicity and lower first-request latency.
- Should there be a connection health check (e.g., periodic ping) to detect a crashed Copilot subprocess between
  completion calls? Recommendation: defer to a simple "respawn on failure" strategy for the MVP.
- Should there be a post-MVP heuristic token estimator for Copilot that derives approximate counts from visible
  prompt/response text? Recommendation: only if the estimates are clearly labeled as approximate and kept separate from
  authoritative provider-reported token counts.
