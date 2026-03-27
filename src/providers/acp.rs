//! ACP (Agent Client Protocol) transport layer.
//!
//! This module provides the low-level NDJSON / JSON-RPC 2.0 transport for communicating with
//! the GitHub Copilot CLI when started in `--acp --stdio` mode.
//!
//! # Protocol
//!
//! - Each message is a single JSON object followed by a newline (`\n`) — NDJSON framing.
//! - Requests follow JSON-RPC 2.0: `{ "jsonrpc": "2.0", "id": <u64>, "method": "...", "params": {...} }`.
//! - Responses carry `id` + `result` or `error`.
//! - Notifications carry `method` + `params` but no `id`.
//!
//! # Lifecycle
//!
//! ```text
//! initialize request  → initialize response
//! session/new request → session/new response { sessionId }
//! session/prompt      → session/update notifications (chunks)
//!                     → session/prompt response { stopReason }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

// ────────────────────────────────────────────────────────────────────────────
// JSON-RPC 2.0 wire types
// ────────────────────────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 request sent to the ACP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response received from the ACP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 notification (no `id` field) from the ACP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 request initiated by the ACP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcServerRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Discriminated result of reading one NDJSON line from the ACP server.
#[derive(Debug, Clone)]
pub enum AcpMessage {
    /// A response to a previous request (has `id` + `result`/`error`).
    Response(JsonRpcResponse),
    /// A request initiated by the ACP server (has `id` + `method`).
    Request(JsonRpcServerRequest),
    /// An unsolicited notification from the server (no `id`).
    Notification(JsonRpcNotification),
}

// ────────────────────────────────────────────────────────────────────────────
// ACP parameter / result types
// ────────────────────────────────────────────────────────────────────────────

/// Params for `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_capabilities: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_info: Option<Value>,
}

/// Result of a successful `initialize` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    #[serde(default)]
    pub server_capabilities: Value,
    #[serde(default)]
    pub server_info: Value,
}

/// Params for `session/new`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

/// Result of `session/new`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
}

/// A single content block in a prompt (only text is supported).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

/// Params for `session/prompt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    Refusal,
    #[serde(other)]
    Other,
}

/// Result of `session/prompt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
}

/// Params delivered via a `session/update` notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateParams {
    pub session_id: String,
    #[serde(rename = "type")]
    pub update_type: String,
    #[serde(default)]
    pub content: Option<Value>,
}

/// Params for `session/requestPermission`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionParams {
    pub session_id: String,
    pub permission_id: String,
    #[serde(default)]
    pub details: Value,
}

/// Response to `session/requestPermission`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionResult {
    pub outcome: RequestPermissionOutcome,
}

/// Outcome for a permission request response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionOutcome {
    pub outcome: String,
}

// ────────────────────────────────────────────────────────────────────────────
// ACP Transport
// ────────────────────────────────────────────────────────────────────────────

/// Low-level ACP transport over `tokio::process` stdio streams.
///
/// Writes NDJSON-encoded JSON-RPC 2.0 requests to the child process's stdin and
/// reads NDJSON lines from stdout. Maintains a monotonically increasing request
/// ID counter.
pub struct AcpTransport {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl AcpTransport {
    /// Wrap existing piped stdin/stdout handles into an `AcpTransport`.
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
        }
    }

    /// Serialize `request` as a single JSON line and write it to stdin.
    ///
    /// Returns the request ID that was assigned.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<u64, AcpTransportError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest::new(id, method, params);
        let mut line = serde_json::to_string(&req).map_err(AcpTransportError::Serialization)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(AcpTransportError::Io)?;
        self.stdin.flush().await.map_err(AcpTransportError::Io)?;
        Ok(id)
    }

    /// Read one NDJSON line from stdout and deserialize it as an [`AcpMessage`].
    ///
    /// Returns `None` when the process closes its stdout (EOF).
    pub async fn read_message(&mut self) -> Result<Option<AcpMessage>, AcpTransportError> {
        let line = self
            .stdout
            .next_line()
            .await
            .map_err(AcpTransportError::Io)?;

        let line = match line {
            Some(l) => l,
            None => return Ok(None),
        };

        let line = line.trim();
        if line.is_empty() {
            // Skip blank lines; caller should loop.
            return Ok(None);
        }

        // Peek: if the JSON has an "id" key and a `method`, it is a server request.
        // If it has an `id` without `method`, it is a response. Otherwise it is a notification.
        let raw: Value = serde_json::from_str(line).map_err(AcpTransportError::Deserialization)?;

        if raw.get("id").is_some() && raw.get("method").is_some() {
            let req: JsonRpcServerRequest =
                serde_json::from_value(raw).map_err(AcpTransportError::Deserialization)?;
            Ok(Some(AcpMessage::Request(req)))
        } else if raw.get("id").is_some() {
            let resp: JsonRpcResponse =
                serde_json::from_value(raw).map_err(AcpTransportError::Deserialization)?;
            Ok(Some(AcpMessage::Response(resp)))
        } else {
            let notif: JsonRpcNotification =
                serde_json::from_value(raw).map_err(AcpTransportError::Deserialization)?;
            Ok(Some(AcpMessage::Notification(notif)))
        }
    }

    // ── Typed method helpers ─────────────────────────────────────────────

    /// Send `initialize` and return the assigned request ID.
    pub async fn send_initialize(&mut self) -> Result<u64, AcpTransportError> {
        let params = InitializeParams {
            protocol_version: 1,
            client_capabilities: Value::Object(serde_json::Map::new()),
            client_info: None,
        };
        let params_value =
            serde_json::to_value(params).map_err(AcpTransportError::Serialization)?;
        self.send_request("initialize", Some(params_value)).await
    }

    /// Send `session/new` and return the assigned request ID.
    pub async fn send_session_new(&mut self) -> Result<u64, AcpTransportError> {
        let cwd = std::env::current_dir()
            .map_err(AcpTransportError::Io)?
            .into_os_string()
            .into_string()
            .map_err(|p| {
                AcpTransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "current working directory contains non-UTF-8 characters: {}",
                        p.to_string_lossy()
                    ),
                ))
            })?;
        let params = NewSessionParams {
            cwd,
            mcp_servers: vec![],
        };
        let params_value =
            serde_json::to_value(params).map_err(AcpTransportError::Serialization)?;
        self.send_request("session/new", Some(params_value)).await
    }

    /// Send `session/prompt` with the given session ID and prompt text, and return the request ID.
    pub async fn send_session_prompt(
        &mut self,
        session_id: &str,
        prompt_text: &str,
    ) -> Result<u64, AcpTransportError> {
        let params = PromptParams {
            session_id: session_id.to_owned(),
            prompt: vec![ContentBlock::Text {
                text: prompt_text.to_owned(),
            }],
        };
        let params_value =
            serde_json::to_value(params).map_err(AcpTransportError::Serialization)?;
        self.send_request("session/prompt", Some(params_value))
            .await
    }

    /// Send `session/requestPermission` response (always cancels).
    pub async fn send_permission_response(
        &mut self,
        session_id: &str,
        permission_id: &str,
    ) -> Result<u64, AcpTransportError> {
        let result = RequestPermissionResult {
            outcome: RequestPermissionOutcome {
                outcome: "cancelled".to_owned(),
            },
        };
        // ACP uses a notification-style response for permission: send as a regular request.
        let params = serde_json::json!({
            "sessionId": session_id,
            "permissionId": permission_id,
            "outcome": result.outcome,
        });
        self.send_request("session/respondPermission", Some(params))
            .await
    }

    /// Wait for the response to request `expected_id`, processing and returning any
    /// interleaved notifications via `on_notification`.
    ///
    /// Returns the `result` JSON value from the matching response.
    pub async fn wait_for_response(
        &mut self,
        expected_id: u64,
        mut on_notification: impl FnMut(&JsonRpcNotification),
    ) -> Result<Value, AcpTransportError> {
        loop {
            match self.read_message().await? {
                None => {
                    return Err(AcpTransportError::UnexpectedEof);
                }
                Some(AcpMessage::Response(resp)) if resp.id == expected_id => {
                    if let Some(err) = resp.error {
                        return Err(AcpTransportError::RpcError(err));
                    }
                    return Ok(resp.result.unwrap_or(Value::Null));
                }
                Some(AcpMessage::Response(resp)) => {
                    // Response for a different ID — unexpected but not fatal; skip.
                    tracing::warn!(
                        id = resp.id,
                        expected = expected_id,
                        "ACP: received response for unexpected request ID"
                    );
                }
                Some(AcpMessage::Request(req)) => {
                    if req.method == "session/request_permission" {
                        let params: RequestPermissionParams =
                            serde_json::from_value(req.params.clone().unwrap_or(Value::Null))
                                .map_err(AcpTransportError::Deserialization)?;
                        self.send_permission_response(&params.session_id, &params.permission_id)
                            .await?;
                        tracing::warn!(
                            session_id = %params.session_id,
                            permission_id = %params.permission_id,
                            "ACP: permission request refused"
                        );
                    } else {
                        tracing::warn!(
                            id = req.id,
                            method = %req.method,
                            "ACP: received unsupported server request"
                        );
                    }
                }
                Some(AcpMessage::Notification(notif)) => {
                    on_notification(&notif);
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Transport error type
// ────────────────────────────────────────────────────────────────────────────

/// Errors produced by the ACP transport layer.
#[derive(Debug)]
pub enum AcpTransportError {
    /// An I/O error on the subprocess stdio streams.
    Io(std::io::Error),
    /// Failed to serialize a request to JSON.
    Serialization(serde_json::Error),
    /// Failed to deserialize a response from JSON.
    Deserialization(serde_json::Error),
    /// The ACP server returned a JSON-RPC error response.
    RpcError(JsonRpcError),
    /// The subprocess closed stdout before sending the expected response.
    UnexpectedEof,
    /// The `initialize` response reported an incompatible protocol version.
    ProtocolVersionMismatch { expected: u32, got: u32 },
}

impl std::fmt::Display for AcpTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "ACP I/O error: {e}"),
            Self::Serialization(e) => write!(f, "ACP serialization error: {e}"),
            Self::Deserialization(e) => write!(f, "ACP deserialization error: {e}"),
            Self::RpcError(e) => write!(f, "ACP RPC error {}: {}", e.code, e.message),
            Self::UnexpectedEof => write!(f, "ACP unexpected EOF from copilot subprocess"),
            Self::ProtocolVersionMismatch { expected, got } => write!(
                f,
                "ACP protocol version mismatch: expected {expected}, got {got}"
            ),
        }
    }
}

impl std::error::Error for AcpTransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Serialization(e) | Self::Deserialization(e) => Some(e),
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

    #[test]
    fn json_rpc_request_serializes_correctly() {
        let req = JsonRpcRequest::new(1, "initialize", None);
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
        assert!(!json.contains("params")); // None is skipped
    }

    #[test]
    fn json_rpc_request_with_params_serializes_correctly() {
        let params = serde_json::json!({ "protocolVersion": 1 });
        let req = JsonRpcRequest::new(2, "initialize", Some(params));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"id\":2"));
    }

    #[test]
    fn json_rpc_response_deserializes_with_result() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, 1);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn json_rpc_response_deserializes_with_error() {
        let json =
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, 2);
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn json_rpc_notification_deserializes() {
        let json = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","type":"agent_message_chunk","content":{"text":"hello"}}}"#;
        let notif: JsonRpcNotification = serde_json::from_str(json).unwrap();
        assert_eq!(notif.method, "session/update");
        assert!(notif.params.is_some());
    }

    #[test]
    fn json_rpc_server_request_deserializes() {
        let json = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"s1","permissionId":"perm-1","details":{}}}"#;
        let req: JsonRpcServerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, 7);
        assert_eq!(req.method, "session/request_permission");
        assert!(req.params.is_some());
    }

    #[test]
    fn request_permission_params_deserialize_correctly() {
        let json = r#"{"sessionId":"session-1","permissionId":"perm-1","details":{"kind":"fs"}}"#;
        let params: RequestPermissionParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.session_id, "session-1");
        assert_eq!(params.permission_id, "perm-1");
        assert_eq!(params.details["kind"], "fs");
    }

    #[test]
    fn acp_message_discriminates_response_by_id_field() {
        let response_json = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let notification_json = r#"{"jsonrpc":"2.0","method":"session/update","params":null}"#;
        let request_json =
            r#"{"jsonrpc":"2.0","id":2,"method":"session/request_permission","params":null}"#;

        let resp_val: Value = serde_json::from_str(response_json).unwrap();
        let notif_val: Value = serde_json::from_str(notification_json).unwrap();
        let req_val: Value = serde_json::from_str(request_json).unwrap();

        assert!(resp_val.get("id").is_some());
        assert!(notif_val.get("id").is_none());
        assert!(req_val.get("id").is_some());
        assert!(req_val.get("method").is_some());
    }

    #[test]
    fn stop_reason_deserializes_end_turn() {
        let json = r#""end_turn""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::EndTurn);
    }

    #[test]
    fn stop_reason_deserializes_refusal() {
        let json = r#""refusal""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::Refusal);
    }

    #[test]
    fn stop_reason_deserializes_unknown_as_other() {
        let json = r#""something_new""#;
        let reason: StopReason = serde_json::from_str(json).unwrap();
        assert_eq!(reason, StopReason::Other);
    }

    #[test]
    fn initialize_params_serializes_with_empty_capabilities() {
        let params = InitializeParams {
            protocol_version: 1,
            client_capabilities: Value::Object(serde_json::Map::new()),
            client_info: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"protocolVersion\":1"));
        assert!(json.contains("\"clientCapabilities\":{}"));
    }

    #[test]
    fn new_session_params_serializes_with_cwd_and_empty_mcp_servers() {
        let params = NewSessionParams {
            cwd: "/home/user/project".to_owned(),
            mcp_servers: vec![],
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("\"cwd\":\"/home/user/project\""));
        assert!(json.contains("\"mcpServers\":[]"));
    }

    #[test]
    fn content_block_text_serializes_with_type_tag() {
        let block = ContentBlock::Text {
            text: "hello world".to_owned(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"hello world\""));
    }

    #[test]
    fn prompt_result_deserializes_stop_reason() {
        let json = r#"{"stopReason":"end_turn"}"#;
        let result: PromptResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn acp_transport_error_display_formats_correctly() {
        let err = AcpTransportError::UnexpectedEof;
        assert!(err.to_string().contains("unexpected EOF"));

        let rpc_err = AcpTransportError::RpcError(JsonRpcError {
            code: -32600,
            message: "Invalid Request".to_owned(),
            data: None,
        });
        assert!(rpc_err.to_string().contains("-32600"));
        assert!(rpc_err.to_string().contains("Invalid Request"));

        let version_err = AcpTransportError::ProtocolVersionMismatch {
            expected: 1,
            got: 2,
        };
        assert!(version_err.to_string().contains("mismatch"));
    }
}
