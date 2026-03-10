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
  - Implement the ACP client-side protocol lifecycle: `initialize` -> `session/new` -> `session/prompt` -> cleanup.
  - Parse NDJSON-framed JSON-RPC 2.0 messages from the Copilot process stdout.
  - Implement `rig`'s `CompletionModel` trait so the Copilot provider is interchangeable with OpenAI/Anthropic/Gemini.
  - Register `"copilot"` as a valid provider name in the existing provider factory.
  - Map ACP errors and subprocess failures into the established `TradingError` hierarchy.
  - Handle graceful shutdown of the Copilot subprocess.

- **Non-Goals:**
  - Implementing ACP client capabilities (`fs/read_text_file`, `fs/write_text_file`, `terminal/*`) — the Copilot
    provider acts as a minimal ACP client that does not grant file system or terminal access to the Copilot agent.
  - Supporting `session/load` (session resumption) — each `rig` completion call creates a fresh ACP session.
  - Implementing MCP server connections — no MCP servers are passed to `session/new`.
  - Supporting image, audio, or embedded context content blocks — only text prompts are sent.
  - Implementing the `session/request_permission` handler beyond refusing all permission requests (Copilot should not
    execute tools on our behalf; we are using it purely as a reasoning engine).
  - Streaming partial responses to callers — the provider collects the full response before returning to `rig`.

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
- `CopilotClient`: manages the lifecycle of the Copilot CLI subprocess. Spawns the process on first use (lazy
  initialization), sends `initialize`, and maintains the `AcpTransport` handle. Provides `Drop`-based cleanup to
  terminate the subprocess.
- `CopilotCompletionModel`: implements `rig::completion::CompletionModel`. On each `completion()` call:
  1. Ensures the `CopilotClient` is initialized (lazy spawn + ACP initialize if first call).
  2. Sends `session/new` to create a fresh session.
  3. Translates the `rig` completion request (system prompt + user message) into an ACP `session/prompt` call.
  4. Processes `session/update` notifications, accumulating `agent_message_chunk` text content.
  5. Waits for the `session/prompt` response (with `stopReason`).
  6. Assembles the accumulated text into a `rig` `CompletionResponse`.
  7. Does NOT terminate the subprocess between calls — the client persists for reuse.

### ACP Protocol Lifecycle Per Completion

```
  CopilotCompletionModel::completion(request)
       │
       ├── (first call only) spawn `copilot --acp --stdio`
       ├── (first call only) send `initialize` { protocolVersion: 1, clientCapabilities: {} }
       ├── receive `initialize` response (negotiate version)
       │
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

- **Lazy spawn**: The Copilot subprocess is not started at application startup. It is spawned on the first completion
  request to avoid resource consumption when the Copilot provider is configured but not used (e.g., the other tier
  uses a different provider).
- **Persistent process**: Once spawned, the subprocess persists across multiple completion calls within the same
  application run. This avoids the overhead of repeated process spawn + ACP initialization for every agent call.
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

- **Decision: Collect full response before returning** — We accumulate all `agent_message_chunk` notifications into
  a single response string rather than streaming. The `rig` `CompletionModel` trait expects a complete response, and
  the downstream agents parse structured JSON from the response.
  - *Alternatives considered:* Streaming would reduce time-to-first-token but is incompatible with `rig`'s
    synchronous completion model and the requirement for structured JSON output parsing.

- **Decision: Lazy subprocess spawn** — The Copilot process is only started when the first completion request arrives,
  not at application startup.
  - *Alternatives considered:* Eager spawn at startup would provide faster first-call latency, but wastes resources
    when Copilot is not the active provider for either tier.

- **Decision: Factory registration via `"copilot"` provider string** — The `add-copilot-provider` change adds a
  `"copilot"` match arm to the existing provider factory. This is the only modification to `factory.rs`.
  - *Alternatives considered:* A plugin registry system would be more extensible but overengineered for a single
    additional provider.

## Risks / Trade-offs

- **Copilot CLI must be installed and authenticated** — If the `copilot` binary is not on `$PATH` or the user is not
  authenticated, the subprocess spawn fails. Mitigation: the provider returns a clear `TradingError::Rig` with a
  diagnostic message indicating the CLI is missing or auth is required. The `config check` CLI command (from
  `add-cli`) can validate Copilot availability at startup.

- **ACP protocol is in public preview** — The protocol may change. Mitigation: the ACP transport layer (`acp.rs`)
  isolates all protocol-specific logic; breaking changes are contained to that module. We pin to protocol version 1.

- **No token usage metadata from ACP** — The ACP protocol does not expose token counts in prompt responses. The
  `TokenUsageTracker` will record zero/unknown for prompt/completion tokens when using the Copilot provider, and
  record wall-clock latency only. Mitigation: document this limitation; latency tracking remains functional.

- **Subprocess management complexity** — Managing a child process (spawn, stdio piping, graceful shutdown, crash
  recovery) adds complexity compared to HTTP-based providers. Mitigation: the `AcpTransport` struct encapsulates
  all process management; the `CopilotCompletionModel` only interacts through typed method calls.

- **Copilot response time variance** — Copilot CLI may have higher latency than direct API calls. Mitigation:
  the same `agent_timeout_secs` timeout from the provider layer applies; if Copilot exceeds the deadline, the
  request fails with `TradingError::NetworkTimeout` like any other provider.

## Open Questions

- Should the Copilot CLI executable path be configurable via `ApiConfig` (e.g., `copilot_cli_path`) or always
  resolved from `$PATH`? Recommendation: make it configurable with a default of `"copilot"`.
- Should there be a connection health check (e.g., periodic ping) to detect a crashed Copilot subprocess between
  completion calls? Recommendation: defer to a simple "respawn on failure" strategy for the MVP.
