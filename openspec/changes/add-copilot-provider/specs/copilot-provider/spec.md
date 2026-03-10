# `copilot-provider` Capability

## ADDED Requirements

### Requirement: ACP Transport Layer

The system MUST provide an ACP transport module (`src/providers/acp.rs`) that manages NDJSON-framed JSON-RPC 2.0
communication over stdio streams with a Copilot CLI subprocess. The transport MUST serialize outgoing JSON-RPC requests
as single-line NDJSON (one JSON object per line, terminated by `\n`) to the subprocess stdin, and deserialize incoming
JSON-RPC responses and notifications from the subprocess stdout using the same NDJSON framing. The transport MUST
maintain a monotonically increasing JSON-RPC request ID sequence to correlate requests with responses. The transport
MUST define `serde`-serializable types for JSON-RPC 2.0 requests, responses, and notifications conforming to the
JSON-RPC 2.0 specification.

#### Scenario: Send And Receive JSON-RPC Message

- **WHEN** the transport sends a JSON-RPC request with ID 1 to the Copilot subprocess stdin
- **THEN** the message is serialized as a single NDJSON line, and the transport reads the corresponding JSON-RPC
  response with matching ID 1 from stdout

#### Scenario: Interleaved Notifications And Responses

- **WHEN** the Copilot subprocess emits multiple `session/update` notifications followed by a `session/prompt`
  response on stdout
- **THEN** the transport correctly distinguishes notifications (no `id` field) from responses (with `id` field) and
  delivers each to the appropriate handler

#### Scenario: Malformed NDJSON Line

- **WHEN** the Copilot subprocess emits a stdout line that is not valid JSON
- **THEN** the transport returns a `TradingError::Rig` with context indicating an NDJSON parse failure from the
  Copilot provider

### Requirement: Copilot Subprocess Management

The system MUST spawn the Copilot CLI as a child process using `copilot --acp --stdio` (or a configurable executable
path) with stdin, stdout, and stderr piped. The subprocess MUST be spawned lazily on the first completion request,
not at application startup. Once spawned, the subprocess MUST persist across multiple completion calls within the
same application run to amortize process startup and ACP initialization costs. On shutdown (explicit or via `Drop`),
the system MUST close the subprocess stdin to signal EOF, then send SIGTERM if the process does not exit within a
configurable timeout (default 2 seconds). If the subprocess crashes or becomes unresponsive between completion calls,
the system MUST detect the failure and attempt to respawn the process on the next completion request.

#### Scenario: Lazy Spawn On First Completion

- **WHEN** the Copilot provider is configured but no completion request has been made yet
- **THEN** no Copilot subprocess exists, and system resources are not consumed by the provider

#### Scenario: Process Reuse Across Calls

- **WHEN** a second completion request arrives after a successful first completion
- **THEN** the existing Copilot subprocess is reused without spawning a new process or re-running ACP initialization

#### Scenario: Graceful Shutdown

- **WHEN** the application shuts down while a Copilot subprocess is running
- **THEN** the system closes stdin, waits up to the configured timeout for the process to exit, and sends SIGTERM
  if the process has not exited

#### Scenario: Subprocess Crash Recovery

- **WHEN** the Copilot subprocess has crashed and a new completion request arrives
- **THEN** the system detects the dead process, spawns a fresh subprocess, re-initializes the ACP connection, and
  retries the completion request transparently

#### Scenario: Copilot CLI Not Found

- **WHEN** the configured Copilot executable path does not exist or is not on `$PATH`
- **THEN** the system returns a `TradingError::Rig` with a diagnostic message indicating the Copilot CLI binary
  could not be found

### Requirement: ACP Protocol Lifecycle

The system MUST implement the ACP client-side protocol lifecycle for each completion call. On first use of a spawned
subprocess, the system MUST send an `initialize` request with `protocolVersion: 1` and empty `clientCapabilities`
(no `fs`, no `terminal`), and MUST verify that the agent responds with a compatible protocol version. For each
completion call, the system MUST send a `session/new` request with the current working directory and an empty
`mcpServers` list, receiving a `sessionId` in response. The system MUST then send a `session/prompt` request with
the session ID and the prompt text as a `ContentBlock::Text` array. The system MUST NOT send image, audio, or
embedded context content blocks. If the agent sends a `session/request_permission` method call, the system MUST
respond with `{ outcome: { outcome: "cancelled" } }` to refuse all permission requests.

#### Scenario: Successful Initialize Handshake

- **WHEN** the Copilot subprocess is spawned and the system sends an `initialize` request with `protocolVersion: 1`
- **THEN** the subprocess responds with an `initialize` response containing a compatible `protocolVersion`, and
  the system proceeds to session creation

#### Scenario: Protocol Version Mismatch

- **WHEN** the Copilot subprocess responds to `initialize` with a `protocolVersion` that the client does not support
- **THEN** the system returns a `TradingError::Rig` indicating a protocol version incompatibility and terminates
  the subprocess

#### Scenario: Session Creation Per Completion

- **WHEN** a completion request is processed
- **THEN** the system creates a new ACP session via `session/new` and uses the returned `sessionId` for the prompt
  exchange, ensuring no state leaks between unrelated agent calls

#### Scenario: Permission Request Refused

- **WHEN** the Copilot agent sends a `session/request_permission` request during prompt processing
- **THEN** the system responds with a cancelled outcome and continues processing the prompt without granting any
  file system or terminal access

### Requirement: Rig CompletionModel Trait Implementation

The system MUST provide a `CopilotCompletionModel` struct that implements `rig`'s `CompletionModel` trait. The
`completion()` method MUST translate the incoming `rig` completion request (system prompt and user message) into an
ACP `session/prompt` call, accumulate all `agent_message_chunk` text content from `session/update` notifications into
a single response string, and return the assembled text as a `rig` `CompletionResponse` when the `session/prompt`
response arrives with a `stopReason`. The implementation MUST NOT stream partial results to callers. The Copilot
completion model MUST be usable anywhere a `rig` `CompletionModel` is accepted, including the existing agent builder
helper and retry-wrapped completion helpers from the `llm-providers` capability.

#### Scenario: Successful Completion Via Copilot

- **WHEN** an agent sends a completion request through the Copilot provider with a system prompt and user message
- **THEN** the provider returns a complete text response assembled from ACP `agent_message_chunk` notifications,
  compatible with `rig`'s `CompletionResponse` type

#### Scenario: Copilot Used In Agent Builder

- **WHEN** a downstream agent is constructed using the `build_agent` helper with the Copilot completion model
- **THEN** the agent operates identically to one constructed with an OpenAI or Anthropic model, using the same
  prompt/chat helpers and retry logic

#### Scenario: Stop Reason Handling

- **WHEN** the ACP `session/prompt` response contains a `stopReason` of `end_turn`
- **THEN** the provider returns the accumulated response text as a successful completion

#### Scenario: Refusal Stop Reason

- **WHEN** the ACP `session/prompt` response contains a `stopReason` of `refusal`
- **THEN** the provider returns a `TradingError::Rig` indicating the Copilot agent refused the request

### Requirement: Provider Factory Registration

The provider factory MUST accept `"copilot"` as a valid provider name in `LlmConfig.quick_thinking_provider` or
`LlmConfig.deep_thinking_provider`. When `"copilot"` is selected for a tier, the factory MUST construct and return
a `CopilotCompletionModel` instance. The factory MUST NOT require an API key for the Copilot provider (authentication
is handled by the Copilot CLI's own auth mechanism). The Copilot provider MUST integrate with the existing
`prompt_with_retry` and `chat_with_retry` helpers without special-case logic in the retry layer.

#### Scenario: Selecting Copilot As Deep-Thinking Provider

- **WHEN** `LlmConfig.deep_thinking_provider` is set to `"copilot"` and a deep-thinking tier completion is requested
- **THEN** the factory returns a `CopilotCompletionModel` and the completion executes through the ACP protocol

#### Scenario: Copilot With Retry Helper

- **WHEN** a completion request to the Copilot provider fails with a transient error (subprocess temporarily
  unresponsive)
- **THEN** the `prompt_with_retry` helper retries the request using the same backoff policy as other providers

#### Scenario: No API Key Required

- **WHEN** the provider is set to `"copilot"` and no `copilot_api_key` is present in `ApiConfig`
- **THEN** the factory constructs the Copilot provider successfully, relying on the CLI's own authentication

### Requirement: Error Mapping

All Copilot-specific errors MUST be mapped into the established `TradingError` hierarchy. Subprocess spawn failures,
ACP protocol errors (malformed responses, unexpected message types, protocol version mismatches), and connection
failures (closed stdin/stdout, process crash) MUST be mapped to `TradingError::Rig` with context including the
provider name (`"copilot"`), a bounded error summary, and no raw prompt text or response bodies. If the Copilot
agent returns a response that cannot be parsed as the expected structured output, the error MUST be mapped to
`TradingError::SchemaViolation` by the existing provider layer parsing logic (not by the Copilot module itself).

#### Scenario: Subprocess Spawn Failure

- **WHEN** `copilot --acp --stdio` fails to spawn (binary not found, permission denied)
- **THEN** the system returns `TradingError::Rig` with provider name `"copilot"` and a human-readable diagnostic

#### Scenario: ACP Protocol Error

- **WHEN** the Copilot subprocess returns a JSON-RPC error response to a `session/new` request
- **THEN** the system returns `TradingError::Rig` with the JSON-RPC error code and message included in the context

#### Scenario: Structured Output Parsing Failure

- **WHEN** the Copilot provider returns valid text that does not match the expected JSON schema for an agent's
  structured output
- **THEN** the existing provider layer parsing logic returns `TradingError::SchemaViolation` (unchanged from the
  `llm-providers` capability behavior)

### Requirement: Token Usage Reporting

The Copilot provider MUST report completion metadata to the `TokenUsageTracker` via the same interface used by other
providers. Because the ACP protocol does not expose token counts in prompt responses, the Copilot provider MUST
report zero for `prompt_tokens` and `completion_tokens`, and MUST report accurate wall-clock `latency_ms` for each
completion call. The `model_id` field MUST be set to `"copilot"` (or the configured model identifier if the ACP
response includes agent info). The `agent_name` field is set by the calling agent, not by the provider.

#### Scenario: Token Usage With Zero Tokens

- **WHEN** a completion call through the Copilot provider succeeds
- **THEN** the `AgentTokenUsage` record contains `prompt_tokens: 0`, `completion_tokens: 0`, `total_tokens: 0`,
  a valid `latency_ms` value, and `model_id` set to `"copilot"`

### Requirement: Module Boundary

The Copilot provider implementation MUST be contained within `src/providers/copilot.rs` and `src/providers/acp.rs`.
These files MUST NOT modify foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`). The only
modification to the existing `llm-providers` code is the addition of a `"copilot"` match arm in the provider factory
function in `src/providers/factory.rs`. The `copilot` and `acp` modules MUST be re-exported through
`src/providers/mod.rs` (stubs already declared by `add-project-foundation`).

#### Scenario: No Foundation File Modifications

- **WHEN** the `add-copilot-provider` change is implemented
- **THEN** no files in `src/config.rs`, `src/error.rs`, or `src/state/` are modified

#### Scenario: Factory Registration Is Minimal

- **WHEN** the `"copilot"` provider is registered in the factory
- **THEN** the change to `src/providers/factory.rs` consists only of adding a match arm that constructs a
  `CopilotCompletionModel`, with no changes to the factory's public interface or other provider branches
