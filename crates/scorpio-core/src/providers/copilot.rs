//! GitHub Copilot provider via ACP (Agent Client Protocol).
//!
//! This module implements the `rig` [`CompletionModel`] trait for GitHub Copilot by spawning the
//! Copilot CLI as an ACP server over stdio and translating `rig` completion requests into
//! JSON-RPC 2.0 ACP calls.
//!
//! # Architecture
//!
//! - [`CopilotClient`] manages the child process lifecycle (spawn, initialize, crash recovery,
//!   graceful shutdown).
//! - [`CopilotCompletionModel`] wraps a shared [`CopilotClient`] behind an async mutex and
//!   implements `rig`'s [`CompletionModel`] + [`CompletionClient`] traits.
//!
//! # Token Usage
//!
//! ACP does not expose authoritative token counts from the Copilot backend.  All token fields
//! in the returned [`Usage`] struct are set to `0` as an explicit "unavailable" sentinel.
//! Do **not** treat these zeros as measured counts.  Any future heuristic estimate derived from
//! visible text would be approximate-only and is deferred to a post-MVP phase.
//!
//! # Concurrency
//!
//! A single Copilot process is protected by a `tokio::sync::Mutex` so concurrent callers
//! cannot interleave NDJSON messages.  Copilot-backed requests are serialized; this is
//! intentional for MVP correctness.

use std::sync::Arc;
use std::time::Instant;

use rig::completion::{
    AssistantContent, CompletionError, CompletionRequest, CompletionResponse, Usage,
};
use rig::message::{Message, Text};
use rig::streaming::StreamingCompletionResponse;
use rig::{OneOrMany, client::FinalCompletionResponse};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::providers::acp::{
    AcpTransport, AcpTransportError, InitializeResult, JsonRpcNotification, NewSessionResult,
    PromptResult, StopReason,
};

async fn terminate_child_process(child: &mut tokio::process::Child) {
    let _ = child.stdin.take();
    match tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

fn abort_stderr_task(stderr_task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = stderr_task.take() {
        task.abort();
    }
}

async fn cleanup_failed_startup(
    child: &mut tokio::process::Child,
    stderr_task: &mut Option<tokio::task::JoinHandle<()>>,
) {
    abort_stderr_task(stderr_task);
    terminate_child_process(child).await;
}

// ────────────────────────────────────────────────────────────────────────────
// Sentinel "raw response" type
// ────────────────────────────────────────────────────────────────────────────

/// Placeholder raw-response type for the Copilot completion model.
///
/// ACP does not return a typed provider-specific response object, so we capture only the
/// latency observed on our side.  Token counts are intentionally zero (unavailable from ACP).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotRawResponse {
    /// Wall-clock milliseconds observed between sending `session/prompt` and receiving the
    /// `session/prompt` response.  Approximate: does not include ACP handshake overhead.
    pub latency_ms: u128,
}

impl rig::completion::GetTokenUsage for CopilotRawResponse {
    fn token_usage(&self) -> Option<Usage> {
        // Token usage is not available from ACP.
        None
    }
}

// ────────────────────────────────────────────────────────────────────────────
// CopilotClient — subprocess lifecycle
// ────────────────────────────────────────────────────────────────────────────

/// Manages the lifecycle of a `copilot --acp --stdio` child process.
///
/// Call [`CopilotClient::ensure_initialized`] before each use.  The client keeps the
/// subprocess alive across calls to avoid re-initialization overhead.
pub struct CopilotClient {
    /// Path to the Copilot CLI executable (default: `"copilot"`).
    exe_path: String,
    /// The model to request from the Copilot CLI (e.g. `"claude-haiku-4.5"`).
    model_id: String,
    /// Live ACP transport, present when the subprocess is running and initialized.
    transport: Option<AcpTransport>,
    /// Handle to the child process (needed for graceful shutdown).
    child: Option<tokio::process::Child>,
    /// Background stderr drainer task for the child process.
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl CopilotClient {
    /// Create a new client with the given executable path and model.
    pub fn new(exe_path: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            exe_path: exe_path.into(),
            model_id: model_id.into(),
            transport: None,
            child: None,
            stderr_task: None,
        }
    }

    /// The model ID this client is configured for.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Poll the child process status non-blockingly and return `true` if it is still running.
    ///
    /// Requires `&mut self` because [`tokio::process::Child::try_wait`] takes `&mut self`.
    /// Named `check_alive` rather than `is_alive` to signal that the call is not a pure
    /// predicate — it advances the process-wait state machine.
    pub fn check_alive(&mut self) -> bool {
        match self.child.as_mut() {
            None => false,
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    debug!(?status, "copilot child already exited");
                    false
                }
                Err(error) => {
                    warn!(error = %error, "failed to poll copilot child status; treating process as dead");
                    false
                }
            },
        }
    }

    /// Ensure the ACP client is ready: spawn (or respawn) the process and send `initialize`.
    ///
    /// If the process is already running and initialized this is a no-op.  If the process has
    /// died the old handles are dropped and the process is respawned.
    pub async fn ensure_initialized(&mut self) -> Result<(), CopilotError> {
        if self.transport.is_some() && self.check_alive() {
            return Ok(());
        }

        // Drop stale handles.
        self.reset_process_state().await;

        let mut child = tokio::process::Command::new(&self.exe_path)
            .args(["--acp", "--stdio", "--model", &self.model_id])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                CopilotError::SpawnFailed(format!("failed to spawn '{}': {e}", self.exe_path))
            })?;

        let mut stderr_task = None;

        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::SpawnFailed("no stdin handle".to_owned()));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::SpawnFailed("no stdout handle".to_owned()));
            }
        };

        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::SpawnFailed("no stderr handle".to_owned()));
            }
        };

        stderr_task = Some(tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let truncated = if line.len() > 200 {
                    format!("{}...", line.chars().take(200).collect::<String>())
                } else {
                    line
                };
                debug!(stderr = %truncated, "copilot stderr");
            }
        }));

        let mut transport = AcpTransport::new(stdin, stdout);

        let req_id = match transport.send_initialize().await {
            Ok(req_id) => req_id,
            Err(error) => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::Transport(error));
            }
        };

        let result_value = match transport.wait_for_response(req_id, |_notif| {}).await {
            Ok(result_value) => result_value,
            Err(error) => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::Transport(error));
            }
        };

        let init_result: InitializeResult = match serde_json::from_value(result_value) {
            Ok(init_result) => init_result,
            Err(error) => {
                cleanup_failed_startup(&mut child, &mut stderr_task).await;
                return Err(CopilotError::ProtocolError(format!(
                    "failed to parse initialize result: {error}"
                )));
            }
        };

        const EXPECTED_VERSION: u32 = 1;
        if init_result.protocol_version != EXPECTED_VERSION {
            cleanup_failed_startup(&mut child, &mut stderr_task).await;
            return Err(CopilotError::Transport(
                AcpTransportError::ProtocolVersionMismatch {
                    expected: EXPECTED_VERSION,
                    got: init_result.protocol_version,
                },
            ));
        }

        debug!(
            protocol_version = init_result.protocol_version,
            "Copilot ACP initialized"
        );

        self.transport = Some(transport);
        self.stderr_task = stderr_task;
        self.child = Some(child);
        Ok(())
    }

    async fn reset_process_state(&mut self) {
        self.transport = None;

        abort_stderr_task(&mut self.stderr_task);

        if let Some(mut child) = self.child.take() {
            terminate_child_process(&mut child).await;
        }
    }

    /// Execute a completion via ACP: `session/new` → `session/prompt` → accumulate chunks.
    ///
    /// Returns the assembled response text and observed latency.
    pub async fn complete(&mut self, prompt_text: &str) -> Result<(String, u128), CopilotError> {
        // Ensure transport is live (caller should have called ensure_initialized, but be safe).
        self.ensure_initialized().await?;

        let transport = self.transport.as_mut().ok_or_else(|| {
            CopilotError::ProtocolError("transport not available after init".to_owned())
        })?;

        // ── 1. session/new ───────────────────────────────────────────────
        let new_req_id = transport
            .send_session_new()
            .await
            .map_err(CopilotError::Transport)?;

        let new_result_value = transport
            .wait_for_response(new_req_id, |_| {})
            .await
            .map_err(CopilotError::Transport)?;

        let new_result: NewSessionResult =
            serde_json::from_value(new_result_value).map_err(|e| {
                CopilotError::ProtocolError(format!("failed to parse session/new result: {e}"))
            })?;

        let session_id = new_result.session_id;
        debug!(session_id = %session_id, "ACP session created");

        // ── 2. session/prompt ────────────────────────────────────────────
        let started_at = Instant::now();

        let prompt_req_id = transport
            .send_session_prompt(&session_id, prompt_text)
            .await
            .map_err(CopilotError::Transport)?;

        // ── 3. Accumulate chunks until session/prompt response ───────────
        let mut accumulated_text = String::new();
        let session_id_clone = session_id.clone();

        let prompt_result_value = {
            let transport = self.transport.as_mut().unwrap();
            transport
                .wait_for_response(prompt_req_id, |notif: &JsonRpcNotification| {
                    handle_session_update(notif, &session_id_clone, &mut accumulated_text);
                })
                .await
                .map_err(CopilotError::Transport)?
        };

        let latency_ms = started_at.elapsed().as_millis();

        let prompt_result: PromptResult =
            serde_json::from_value(prompt_result_value).map_err(|e| {
                CopilotError::ProtocolError(format!("failed to parse session/prompt result: {e}"))
            })?;

        // Map stop reason to success or error.
        match prompt_result.stop_reason {
            Some(StopReason::Refusal) => {
                return Err(CopilotError::Refusal);
            }
            Some(StopReason::EndTurn) | Some(StopReason::Other) | None => {}
        }

        Ok((accumulated_text, latency_ms))
    }

    pub async fn shutdown(&mut self) {
        self.reset_process_state().await;
    }
}

/// Process a `session/update` notification, accumulating text chunks.
fn handle_session_update(notif: &JsonRpcNotification, session_id: &str, accumulated: &mut String) {
    if notif.method != "session/update" {
        return;
    }

    let params = match &notif.params {
        Some(p) => p,
        None => return,
    };

    // Validate session ID matches if present.
    if let Some(sid) = params.get("sessionId").and_then(|v| v.as_str())
        && sid != session_id
    {
        return;
    }

    let update_type = params.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match update_type {
        "agent_message_chunk" => {
            // Content may be { "text": "..." } or just a string.
            if let Some(text) = params
                .get("content")
                .and_then(|c| c.get("text").or(Some(c)))
                .and_then(|v| v.as_str())
            {
                accumulated.push_str(text);
            }
        }
        "tool_call" => {
            warn!("ACP: unexpected tool_call notification (Copilot should not use tools)");
        }
        "request_permission" => {
            // Permission requests are handled by the transport layer (cancelled).
            // Logging here for visibility.
            warn!("ACP: permission request notification received (will be cancelled)");
        }
        other => {
            debug!(update_type = other, "ACP: unrecognised session/update type");
        }
    }
}

impl Drop for CopilotClient {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
        self.transport = None;
        self.child = None;
    }
}

impl std::fmt::Debug for CopilotClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // AcpTransport and tokio::process::Child do not implement Debug, so we format
        // only the fields that are safe to print.
        f.debug_struct("CopilotClient")
            .field("exe_path", &self.exe_path)
            .field("model_id", &self.model_id)
            .field("transport_active", &self.transport.is_some())
            .field("child_active", &self.child.is_some())
            .finish()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// CopilotCompletionModel — rig trait implementation
// ────────────────────────────────────────────────────────────────────────────

/// A shared, cloneable handle to a [`CopilotClient`] protected by an async mutex.
///
/// Implements `rig`'s [`CompletionModel`] so it can be used anywhere a rig completion
/// model is expected, including with `Agent::builder` and `prompt_with_retry`.
#[derive(Clone, Debug)]
pub struct CopilotCompletionModel {
    /// Shared client (Arc so cloning is cheap; Mutex serialises ACP I/O).
    client: Arc<Mutex<CopilotClient>>,
    /// The model identifier reported back in responses (informational).
    model_id: String,
}

impl CopilotCompletionModel {
    /// Create a new model backed by the given client.
    pub fn new(client: Arc<Mutex<CopilotClient>>, model_id: impl Into<String>) -> Self {
        Self {
            client,
            model_id: model_id.into(),
        }
    }

    /// Model identifier (always `"copilot"` or a custom label).
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// Build the prompt text from a rig `CompletionRequest`.
///
/// Assembles: optional preamble, documents, output schema, then chat history.
/// Documents and schema are critical for typed prompts — rig injects JSON schema
/// constraints via `documents` and `output_schema` that the model needs to see.
fn build_prompt_text(request: &CompletionRequest) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(preamble) = &request.preamble {
        parts.push(format!("[System]\n{preamble}"));
    }

    // Include documents (schema instructions, context) — rig injects these for
    // typed prompts.  Since Copilot ACP accepts only plain text, we render each
    // document using its Display impl (which produces <file id:...>...</file>).
    if !request.documents.is_empty() {
        let doc_text: String = request
            .documents
            .iter()
            .map(|doc| doc.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("[Documents]\n{doc_text}"));
    }

    // Include output schema instructions when present — this tells the model
    // to return JSON matching the schema, which is critical for typed prompts.
    if let Some(schema) = &request.output_schema {
        match serde_json::to_string_pretty(schema) {
            Ok(schema_json) => {
                parts.push(format!(
                    "[Output Schema]\nYou MUST respond with valid JSON matching this schema:\n{schema_json}"
                ));
            }
            Err(err) => {
                // Serialization of a Schema should never fail in practice, but if it
                // does the model will silently receive no schema constraint — warn loudly
                // so the caller can diagnose the issue.
                tracing::warn!(error = %err, "failed to serialize output_schema; typed prompt will lack schema constraints");
            }
        }
    }

    for msg in request.chat_history.iter() {
        match msg {
            Message::User { content } => {
                let texts: String = content
                    .iter()
                    .filter_map(|uc| {
                        if let rig::message::UserContent::Text(Text { text }) = uc {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !texts.is_empty() {
                    parts.push(format!("[User]\n{texts}"));
                }
            }
            Message::Assistant { content, .. } => {
                let texts: String = content
                    .iter()
                    .filter_map(|ac| {
                        if let AssistantContent::Text(Text { text }) = ac {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !texts.is_empty() {
                    parts.push(format!("[Assistant]\n{texts}"));
                }
            }
            Message::System { content } => {
                if !content.is_empty() {
                    parts.push(format!("[System]\n{content}"));
                }
            }
        }
    }

    parts.join("\n\n")
}

impl rig::completion::CompletionModel for CopilotCompletionModel {
    type Response = CopilotRawResponse;
    type StreamingResponse = FinalCompletionResponse;
    type Client = CopilotProviderClient;

    fn make(client: &Self::Client, model: impl Into<String>) -> Self {
        // CompletionClient::completion_model() calls make() with &Self (the CopilotProviderClient).
        // We clone the underlying model, overriding the model_id label.
        let model_id: String = model.into();
        CopilotCompletionModel {
            client: Arc::clone(&client.model.client),
            model_id,
        }
    }

    fn completion(
        &self,
        request: CompletionRequest,
    ) -> impl std::future::Future<
        Output = Result<CompletionResponse<Self::Response>, CompletionError>,
    > + Send {
        let client = Arc::clone(&self.client);
        let model_id = self.model_id.clone();

        async move {
            let prompt_text = build_prompt_text(&request);

            let (text, latency_ms) = {
                let mut guard = client.lock().await;
                guard.complete(&prompt_text).await.map_err(|e| {
                    CompletionError::ProviderError(format!("provider=copilot model={model_id} {e}"))
                })?
            };

            let raw = CopilotRawResponse { latency_ms };
            let choice = OneOrMany::one(AssistantContent::text(text));

            Ok(CompletionResponse {
                choice,
                // Token usage is not available from ACP — all zeros as sentinel.
                usage: Usage::new(),
                raw_response: raw,
                message_id: None,
            })
        }
    }

    fn stream(
        &self,
        request: CompletionRequest,
    ) -> impl std::future::Future<
        Output = Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError>,
    > + Send {
        // Copilot does not support streaming; collect the full response and wrap it.
        let client = Arc::clone(&self.client);
        let model_id = self.model_id.clone();

        async move {
            let prompt_text = build_prompt_text(&request);

            let (text, _latency_ms) = {
                let mut guard = client.lock().await;
                guard.complete(&prompt_text).await.map_err(|e| {
                    CompletionError::ProviderError(format!("provider=copilot model={model_id} {e}"))
                })?
            };

            // Wrap the complete response as a single-item stream.
            let choice = rig::streaming::RawStreamingChoice::Message(text);
            let stream = futures::stream::once(async move { Ok(choice) });
            Ok(StreamingCompletionResponse::stream(Box::pin(stream)))
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// CopilotProviderClient — rig CompletionClient implementation
// ────────────────────────────────────────────────────────────────────────────

/// A thin provider-client wrapper that satisfies `rig`'s [`CompletionClient`] trait boundary.
///
/// Holds a shared `CopilotCompletionModel`. `completion_model(model_id)` delegates to the
/// default trait impl which calls `CopilotCompletionModel::make()`, creating a new model
/// handle with the requested `model_id`.
#[derive(Clone, Debug)]
pub struct CopilotProviderClient {
    model: CopilotCompletionModel,
}

impl CopilotProviderClient {
    /// Create a provider client wrapping the given shared `CopilotClient`.
    ///
    /// - `exe_path` — path to the Copilot CLI binary (e.g. `"copilot"`).
    /// - `model_id` — model identifier to associate with the default completion model.
    pub fn new(exe_path: impl Into<String>, model_id: impl Into<String>) -> Self {
        let model_id = model_id.into();
        let client = Arc::new(Mutex::new(CopilotClient::new(exe_path, &model_id)));
        let model = CopilotCompletionModel::new(client, model_id);
        Self { model }
    }

    /// Borrow the underlying model.
    pub fn model(&self) -> &CopilotCompletionModel {
        &self.model
    }

    /// Perform the ACP startup preflight: spawn the process and send `initialize`.
    ///
    /// Call this during application startup whenever `"copilot"` is configured as an active
    /// provider tier.  Fails fast if the CLI is not installed, not authenticated, or the ACP
    /// protocol is incompatible.
    pub async fn preflight(&self) -> Result<(), CopilotError> {
        let mut guard = self.model.client.lock().await;
        guard.ensure_initialized().await
    }
}

impl rig::prelude::CompletionClient for CopilotProviderClient {
    type CompletionModel = CopilotCompletionModel;
}

// ────────────────────────────────────────────────────────────────────────────
// Error type
// ────────────────────────────────────────────────────────────────────────────

/// Errors produced by the Copilot provider.
#[derive(Debug)]
pub enum CopilotError {
    /// The Copilot CLI subprocess could not be spawned.
    SpawnFailed(String),
    /// An ACP transport-layer error.
    Transport(AcpTransportError),
    /// The ACP server returned an unexpected or invalid protocol message.
    ProtocolError(String),
    /// The model refused to answer (ACP `stopReason: refusal`).
    Refusal,
}

impl std::fmt::Display for CopilotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(msg) => write!(f, "Copilot spawn failed: {msg}"),
            Self::Transport(e) => write!(f, "Copilot transport error: {e}"),
            Self::ProtocolError(msg) => write!(f, "Copilot protocol error: {msg}"),
            Self::Refusal => write!(f, "Copilot refused to answer the request"),
        }
    }
}

impl std::error::Error for CopilotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(e) => Some(e),
            _ => None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rig::OneOrMany;
    use rig::completion::CompletionRequest;
    use rig::message::{Message, UserContent};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::time::Duration;
    use uuid::Uuid;

    fn make_request(preamble: Option<&str>, messages: Vec<Message>) -> CompletionRequest {
        CompletionRequest {
            model: None,
            preamble: preamble.map(|s| s.to_owned()),
            chat_history: OneOrMany::many(messages).unwrap_or_else(|_| {
                OneOrMany::one(Message::User {
                    content: OneOrMany::one(UserContent::text("placeholder")),
                })
            }),
            documents: vec![],
            tools: vec![],
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
        }
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn unique_temp_path(prefix: &str, suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", Uuid::new_v4()))
    }

    fn write_mock_copilot_script(body: &str) -> PathBuf {
        let script_path = unique_temp_path("copilot-mock", "sh");
        let script = format!("#!/bin/sh\nset -eu\n{body}\n");
        fs::write(&script_path, script).expect("write mock copilot script");
        let mut permissions = fs::metadata(&script_path)
            .expect("stat script")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script_path, permissions).expect("chmod script");
        script_path
    }

    fn process_is_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[test]
    fn build_prompt_includes_preamble() {
        let req = make_request(
            Some("You are a trading analyst."),
            vec![Message::User {
                content: OneOrMany::one(UserContent::text("What is the outlook?")),
            }],
        );
        let prompt = build_prompt_text(&req);
        assert!(prompt.contains("[System]"));
        assert!(prompt.contains("trading analyst"));
        assert!(prompt.contains("[User]"));
        assert!(prompt.contains("What is the outlook?"));
    }

    #[test]
    fn build_prompt_without_preamble() {
        let req = make_request(
            None,
            vec![Message::User {
                content: OneOrMany::one(UserContent::text("hello")),
            }],
        );
        let prompt = build_prompt_text(&req);
        assert!(!prompt.contains("[System]"));
        assert!(prompt.contains("[User]"));
        assert!(prompt.contains("hello"));
    }

    #[test]
    fn build_prompt_includes_chat_history() {
        let req = make_request(
            None,
            vec![
                Message::User {
                    content: OneOrMany::one(UserContent::text("First question")),
                },
                Message::Assistant {
                    content: OneOrMany::one(AssistantContent::text("First answer")),
                    id: None,
                },
                Message::User {
                    content: OneOrMany::one(UserContent::text("Follow-up question")),
                },
            ],
        );
        let prompt = build_prompt_text(&req);
        assert!(prompt.contains("First question"));
        assert!(prompt.contains("First answer"));
        assert!(prompt.contains("Follow-up question"));
        assert!(prompt.contains("[Assistant]"));
    }

    #[test]
    fn copilot_raw_response_token_usage_returns_none() {
        let raw = CopilotRawResponse { latency_ms: 100 };
        use rig::completion::GetTokenUsage;
        assert!(raw.token_usage().is_none());
    }

    #[test]
    fn copilot_error_display_formats_correctly() {
        assert!(CopilotError::Refusal.to_string().contains("refused"));
        assert!(
            CopilotError::SpawnFailed("not found".to_owned())
                .to_string()
                .contains("spawn failed")
        );
        assert!(
            CopilotError::ProtocolError("mismatch".to_owned())
                .to_string()
                .contains("protocol error")
        );
    }

    #[test]
    fn copilot_client_stores_model_id() {
        let client = CopilotClient::new("copilot", "claude-haiku-4.5");
        assert_eq!(client.model_id(), "claude-haiku-4.5");
    }

    #[test]
    fn copilot_client_check_alive_returns_false_when_no_child() {
        let mut client = CopilotClient::new("copilot", "test-model");
        assert!(!client.check_alive());
    }

    #[tokio::test]
    async fn copilot_client_check_alive_returns_true_when_child_running() {
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "sleep 1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep child");

        let mut client = CopilotClient::new("copilot", "test-model");
        client.child = Some(child);

        assert!(client.check_alive());
        client.reset_process_state().await;
    }

    #[tokio::test]
    async fn copilot_client_check_alive_returns_false_when_child_exited() {
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn exiting child");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = CopilotClient::new("copilot", "test-model");
        client.child = Some(child);

        assert!(!client.check_alive());
        client.reset_process_state().await;
    }

    #[tokio::test]
    async fn ensure_initialized_respawns_after_dead_child() {
        let spawn_count_path = unique_temp_path("copilot-spawn-count", "txt");
        let script_path = write_mock_copilot_script(&format!(
            "COUNT_FILE={}\nprintf 'spawn\\n' >> \"$COUNT_FILE\"\nIFS= read -r _ || exit 1\nprintf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":1,\"serverCapabilities\":{{}},\"serverInfo\":{{}}}}}}'\nexit 0",
            shell_quote(&spawn_count_path),
        ));

        let mut client = CopilotClient::new(script_path.display().to_string(), "test-model");
        client
            .ensure_initialized()
            .await
            .expect("first initialize succeeds");

        for _ in 0..20 {
            if !client.check_alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !client.check_alive(),
            "mock copilot process should have exited so respawn path is exercised"
        );

        client
            .ensure_initialized()
            .await
            .expect("second initialize respawns after crash");

        let spawn_count = fs::read_to_string(&spawn_count_path).expect("read spawn count file");
        assert_eq!(
            spawn_count.lines().count(),
            2,
            "expected one initial spawn and one respawn"
        );

        client.shutdown().await;
        let _ = fs::remove_file(script_path);
        let _ = fs::remove_file(spawn_count_path);
    }

    #[tokio::test]
    async fn ensure_initialized_cleans_up_failed_initialize_process() {
        let pid_path = unique_temp_path("copilot-failed-init-pid", "txt");
        let script_path = write_mock_copilot_script(&format!(
            "PID_FILE={}\nprintf '%s\\n' \"$$\" > \"$PID_FILE\"\nIFS= read -r _ || exit 1\nprintf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":999,\"serverCapabilities\":{{}},\"serverInfo\":{{}}}}}}'\nsleep 5",
            shell_quote(&pid_path),
        ));

        let mut client = CopilotClient::new(script_path.display().to_string(), "test-model");
        let error = client
            .ensure_initialized()
            .await
            .expect_err("protocol mismatch should fail initialization");
        assert!(matches!(
            error,
            CopilotError::Transport(AcpTransportError::ProtocolVersionMismatch { .. })
        ));

        let pid: u32 = fs::read_to_string(&pid_path)
            .expect("read pid file")
            .trim()
            .parse()
            .expect("parse pid");

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !process_is_alive(pid),
            "failed initialization should not leave the spawned process running"
        );
        assert!(client.transport.is_none());
        assert!(client.child.is_none());
        assert!(client.stderr_task.is_none());

        let _ = fs::remove_file(script_path);
        let _ = fs::remove_file(pid_path);
    }

    #[test]
    fn copilot_completion_model_is_clone() {
        // Verify that CopilotCompletionModel can be cloned (required by rig's CompletionModel).
        let client = Arc::new(Mutex::new(CopilotClient::new("copilot", "test-model")));
        let model = CopilotCompletionModel::new(client, "copilot");
        let _cloned = model.clone();
    }

    #[test]
    fn handle_session_update_accumulates_text() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: "session/update".to_owned(),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "type": "agent_message_chunk",
                "content": { "text": "hello " }
            })),
        };
        let mut buf = String::new();
        handle_session_update(&notif, "s1", &mut buf);
        assert_eq!(buf, "hello ");

        let notif2 = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: "session/update".to_owned(),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "type": "agent_message_chunk",
                "content": { "text": "world" }
            })),
        };
        handle_session_update(&notif2, "s1", &mut buf);
        assert_eq!(buf, "hello world");
    }

    #[test]
    fn handle_session_update_ignores_wrong_session_id() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: "session/update".to_owned(),
            params: Some(serde_json::json!({
                "sessionId": "other-session",
                "type": "agent_message_chunk",
                "content": { "text": "should be ignored" }
            })),
        };
        let mut buf = String::new();
        handle_session_update(&notif, "s1", &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn handle_session_update_ignores_wrong_method() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: "other/method".to_owned(),
            params: Some(serde_json::json!({})),
        };
        let mut buf = String::new();
        handle_session_update(&notif, "s1", &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn build_prompt_includes_documents() {
        use rig::completion::Document;

        let mut req = make_request(
            Some("System prompt"),
            vec![Message::User {
                content: OneOrMany::one(UserContent::text("Analyze this")),
            }],
        );
        req.documents = vec![Document {
            id: "doc-1".to_owned(),
            text: "Revenue grew 15% YoY".to_owned(),
            additional_props: Default::default(),
        }];

        let prompt = build_prompt_text(&req);
        assert!(
            prompt.contains("Revenue grew 15% YoY"),
            "documents must be included in prompt text, got: {prompt}"
        );
        let system_pos = prompt.find("[System]").unwrap();
        let doc_pos = prompt.find("Revenue grew").unwrap();
        let user_pos = prompt.find("[User]").unwrap();
        assert!(system_pos < doc_pos, "documents should come after preamble");
        assert!(
            doc_pos < user_pos,
            "documents should come before chat history"
        );
    }

    #[test]
    fn completion_model_returns_requested_model_id() {
        use rig::prelude::CompletionClient;
        // CopilotClient::new() is lazy — no subprocess is spawned until preflight().
        let provider = CopilotProviderClient::new("copilot", "default-model");
        let model = provider.completion_model("claude-haiku-4.5");
        assert_eq!(model.model_id(), "claude-haiku-4.5");
    }

    #[test]
    fn build_prompt_includes_output_schema() {
        let mut req = make_request(
            None,
            vec![Message::User {
                content: OneOrMany::one(UserContent::text("Analyze")),
            }],
        );
        req.output_schema = Some(
            serde_json::from_value(serde_json::json!({
                "type": "object",
                "properties": {
                    "decision": { "type": "string" }
                },
                "required": ["decision"]
            }))
            .unwrap(),
        );

        let prompt = build_prompt_text(&req);
        assert!(
            prompt.contains("[Output Schema]"),
            "output schema section must be present, got: {prompt}"
        );
        assert!(
            prompt.contains("\"decision\""),
            "schema content must be included, got: {prompt}"
        );
        let schema_pos = prompt.find("[Output Schema]").unwrap();
        let user_pos = prompt.find("[User]").unwrap();
        assert!(
            schema_pos < user_pos,
            "schema should come before chat history"
        );
    }
}
