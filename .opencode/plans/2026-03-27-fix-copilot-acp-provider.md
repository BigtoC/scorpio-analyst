# Fix Copilot ACP Provider Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the Copilot ACP provider so that model selection, documents, and working directory are correctly communicated to the Copilot CLI, resolving the `-32603` internal error.

**Architecture:** Thread the model ID from config through `CopilotProviderClient` → `CopilotClient` → `--model` CLI argument. Fix `build_prompt_text()` to include rig's `documents` and `output_schema`. Use absolute `cwd` in `session/new`. Fix `completion_model()` to respect the requested model parameter.

**Tech Stack:** Rust, rig-core 0.32, tokio, serde_json

---

## File Structure

| File                       | Responsibility                                | Change                                                                                                                |
|----------------------------|-----------------------------------------------|-----------------------------------------------------------------------------------------------------------------------|
| `src/providers/copilot.rs` | Copilot subprocess lifecycle + rig trait impl | Add `model_id` field to `CopilotClient`, pass `--model` to spawn, fix `build_prompt_text()`, fix `completion_model()` |
| `src/providers/acp.rs`     | ACP transport layer                           | Use absolute `cwd` in `send_session_new()`                                                                            |
| `src/providers/factory.rs` | Provider factory                              | Pass `model_id` to `CopilotProviderClient::new()`                                                                     |

---

## Chunk 1: Thread model_id and fix Copilot provider

### Task 1: Add `model_id` to `CopilotClient` and pass `--model` CLI argument

**Files:**
- Modify: `src/providers/copilot.rs:101-121` (struct + constructor)
- Modify: `src/providers/copilot.rs:149-165` (`ensure_initialized` spawn)
- Modify: `src/providers/copilot.rs:394-403` (Debug impl)

- [ ] **Step 1: Write failing test — `CopilotClient` stores model_id**

Add to `src/providers/copilot.rs` `mod tests`:

```rust
#[test]
fn copilot_client_stores_model_id() {
    let client = CopilotClient::new("copilot", "claude-haiku-4.5");
    assert_eq!(client.model_id(), "claude-haiku-4.5");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test copilot_client_stores_model_id -- --nocapture`
Expected: FAIL — `CopilotClient::new` currently takes only one argument

- [ ] **Step 3: Add `model_id` field to `CopilotClient` and update constructor**

In `src/providers/copilot.rs`, update `CopilotClient` struct (line 101) and `new()` (line 114):

```rust
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
```

Also update the `Debug` impl (line 394) to include `model_id`:

```rust
impl std::fmt::Debug for CopilotClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotClient")
            .field("exe_path", &self.exe_path)
            .field("model_id", &self.model_id)
            .field("transport_active", &self.transport.is_some())
            .field("child_active", &self.child.is_some())
            .finish()
    }
}
```

- [ ] **Step 4: Fix all existing `CopilotClient::new()` call sites to pass model_id**

Update every `CopilotClient::new(single_arg)` to `CopilotClient::new(arg, "test-model")` in tests:

1. `copilot_client_check_alive_returns_false_when_no_child` (line 791): `CopilotClient::new("copilot", "test-model")`
2. `copilot_client_check_alive_returns_true_when_child_running` (line 804): `CopilotClient::new("copilot", "test-model")`
3. `copilot_client_check_alive_returns_false_when_child_exited` (line 823): `CopilotClient::new("copilot", "test-model")`
4. `ensure_initialized_respawns_after_dead_child` (line 838): `CopilotClient::new(script_path, "test-model")`
5. `ensure_initialized_cleans_up_failed_initialize_process` (line 880): `CopilotClient::new(script_path, "test-model")`
6. `copilot_completion_model_is_clone` (line 912): `CopilotClient::new("copilot", "test-model")`

- [ ] **Step 5: Pass `--model` to the spawn command in `ensure_initialized`**

In `src/providers/copilot.rs:157-158`, change:

```rust
// OLD:
.args(["--acp", "--stdio"])

// NEW:
.args(["--acp", "--stdio", "--model", &self.model_id])
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test copilot_client_stores_model_id -- --nocapture`
Expected: PASS

- [ ] **Step 7: Verify existing tests still compile and pass**

Run: `cargo test --lib providers::copilot`
Expected: All existing copilot tests pass

---

### Task 2: Fix `build_prompt_text()` to include documents and output schema

**Files:**
- Modify: `src/providers/copilot.rs:440-485` (`build_prompt_text` function)

- [ ] **Step 1: Write failing test — documents included in prompt**

Add to `src/providers/copilot.rs` `mod tests`:

```rust
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
    assert!(doc_pos < user_pos, "documents should come before chat history");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test build_prompt_includes_documents -- --nocapture`
Expected: FAIL — documents are not currently included

- [ ] **Step 3: Update `build_prompt_text()` to include documents and output schema**

Replace `build_prompt_text` in `src/providers/copilot.rs` (line 440-485):

```rust
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
        if let Ok(schema_json) = serde_json::to_string_pretty(schema) {
            parts.push(format!(
                "[Output Schema]\nYou MUST respond with valid JSON matching this schema:\n{schema_json}"
            ));
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
        }
    }

    parts.join("\n\n")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test build_prompt_includes_documents -- --nocapture`
Expected: PASS

- [ ] **Step 5: Write and run test — output schema included in prompt**

```rust
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
}
```

Run: `cargo test build_prompt_includes_output_schema -- --nocapture`
Expected: PASS

---

### Task 3: Fix `CopilotProviderClient` to accept model_id and respect it in `completion_model()`

**Files:**
- Modify: `src/providers/copilot.rs:571-608`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn completion_model_returns_requested_model_id() {
    use rig::prelude::CompletionClient;
    let provider = CopilotProviderClient::new("copilot", "default-model");
    let model = provider.completion_model("claude-haiku-4.5");
    assert_eq!(model.model_id(), "claude-haiku-4.5");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test completion_model_returns_requested_model_id -- --nocapture`
Expected: FAIL — `CopilotProviderClient::new` takes 1 arg; `completion_model` ignores parameter

- [ ] **Step 3: Update `CopilotProviderClient::new()` and remove `completion_model()` override**

Update `CopilotProviderClient::new()` to accept `model_id`, and **remove** the custom
`completion_model()` override. The default trait impl on `CompletionClient` delegates to
`CopilotCompletionModel::make()` (line 492), which already does the right thing — clones
the `Arc<Mutex<CopilotClient>>` and assigns the requested model_id. Removing the override
eliminates logic duplication.

```rust
impl CopilotProviderClient {
    /// Create a provider client wrapping the given shared `CopilotClient`.
    ///
    /// `exe_path` is the path to the Copilot CLI binary (default: `"copilot"`).
    /// `model_id` is the model to request from the CLI (e.g. `"claude-haiku-4.5"`).
    pub fn new(exe_path: impl Into<String>, model_id: impl Into<String>) -> Self {
        let model_id = model_id.into();
        let client = Arc::new(Mutex::new(CopilotClient::new(exe_path, &model_id)));
        let model = CopilotCompletionModel::new(client, &model_id);
        Self { model }
    }

    // ... model() and preflight() unchanged ...
}

impl rig::prelude::CompletionClient for CopilotProviderClient {
    type CompletionModel = CopilotCompletionModel;

    // No custom completion_model() override — the default trait impl calls
    // CopilotCompletionModel::make(), which correctly clones the shared client
    // and uses the requested model_id.
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test completion_model_returns_requested_model_id -- --nocapture`
Expected: PASS

---

### Task 4: Update factory to pass `model_id` to `CopilotProviderClient::new()`

**Files:**
- Modify: `src/providers/factory.rs:184-228` (`create_provider_client_for`)
- Modify: `src/providers/factory.rs:139-152` (`create_completion_model` — caller)

- [ ] **Step 1: Update `create_provider_client_for` signature to accept `model_id`**

```rust
fn create_provider_client_for(
    provider: ProviderId,
    api_config: &ApiConfig,
    model_id: &str,
) -> Result<ProviderClient, TradingError> {
```

- [ ] **Step 2: Update the Copilot branch to pass `model_id`**

```rust
ProviderId::Copilot => {
    let exe_path = std::env::var("SCORPIO_COPILOT_CLI_PATH")
        .unwrap_or_else(|_| resolve_copilot_exe_path());
    validate_copilot_cli_path(&exe_path)?;
    Ok(ProviderClient::Copilot(CopilotProviderClient::new(
        exe_path, model_id,
    )))
}
```

- [ ] **Step 3: Update callers of `create_provider_client_for`**

In `create_completion_model` (line 146):

```rust
let client = create_provider_client_for(provider, api_config, &model_id)?;
```

- [ ] **Step 4: Run all factory tests**

Run: `cargo test --lib providers::factory`
Expected: All tests pass (non-Copilot branches ignore the extra param)

---

### Task 5: Use absolute `cwd` in `send_session_new()`

**Files:**
- Modify: `src/providers/acp.rs:312-321`

- [ ] **Step 1: Update `send_session_new()` to use absolute cwd**

Propagate `current_dir()` failure as an `AcpTransportError::Io` rather than silently falling
back to `"."` (which was the original bug):

```rust
pub async fn send_session_new(&mut self) -> Result<u64, AcpTransportError> {
    let cwd = std::env::current_dir()
        .map_err(AcpTransportError::Io)?
        .to_string_lossy()
        .into_owned();
    let params = NewSessionParams {
        cwd,
        mcp_servers: vec![],
    };
    let params_value =
        serde_json::to_value(params).map_err(AcpTransportError::Serialization)?;
    self.send_request("session/new", Some(params_value)).await
}
```

- [ ] **Step 2: Replace existing test `new_session_params_serializes_empty_mcp_servers`**

**Replace** (not add alongside) the existing test at `acp.rs:590-598` which hardcodes
`cwd: "."`. The new test uses an absolute path to match the updated behavior:

```rust
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
```

- [ ] **Step 3: Run ACP tests**

Run: `cargo test --lib providers::acp`
Expected: All tests pass

---

### Task 6: Code verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Check formatting**

Run: `cargo fmt -- --check`
Expected: No formatting issues

- [ ] **Step 4: Commit**

```bash
git add src/providers/copilot.rs src/providers/acp.rs src/providers/factory.rs
git commit -m "fix(copilot): thread model_id to CLI, include documents in prompt, use absolute cwd

Root cause: Copilot CLI was spawned without --model flag, causing it to
use an unavailable default model (ACP RPC error -32603). Additionally,
build_prompt_text() dropped rig's documents and output_schema, and
completion_model() discarded the model parameter.

Changes:
- Add model_id field to CopilotClient, pass --model to spawn args
- Include request.documents and output_schema in build_prompt_text()
- Fix CopilotProviderClient::completion_model() to use requested model
- Pass model_id from factory through to CopilotProviderClient::new()
- Use absolute cwd in ACP session/new instead of relative '.'"
```

---

### Task 7: Runtime smoke test via `cargo run`

Verify the full pipeline runs without crashing at startup or during the ACP handshake.
This catches runtime errors (wrong CLI flags, ACP protocol failures, config mismatches)
that unit tests cannot catch.

**Files:** None modified — observation only.

- [ ] **Step 1: Run the binary and capture output**

Run: `cargo run 2>&1 | head -100`

Watch for:
- `failed to preflight configured providers` — ACP handshake or CLI spawn failure
- `ACP RPC error -32603` — the original bug; must not appear
- `failed to spawn` — Copilot CLI not found or bad `--model` flag
- `analysis cycle failed` — pipeline error after a successful start
- Clean progression through `scorpio-analyst initialized` → analyst phase → final decision

Expected: Binary runs, completes preflight successfully, executes at least the analyst
phase, and either prints a final `=== DECISION: ... ===` block or fails with a
domain-level error (e.g. missing Finnhub key, ticker not found) — NOT an ACP or spawn error.

- [ ] **Step 2: Triage any runtime error**

If `cargo run` produces an error:
- **ACP spawn/model error**: Re-check Task 1 Step 5 (`--model` arg) and Task 3
  (`CopilotProviderClient::new` model threading).
- **`-32603` still appears**: Re-check Task 2 (documents/schema in prompt) — the model
  may be rejecting malformed requests.
- **Config error**: Check `config.toml` has valid `quick_thinking_provider = "copilot"`,
  `quick_thinking_model = "claude-haiku-4.5"`, etc.
- **Finnhub/yfinance error**: Non-ACP error; the Copilot fix is working correctly.
  Document the error and consider it out of scope for this fix.

If output is clean (no ACP errors), the fix is confirmed. Record the first ~20 lines of
output here as evidence:

```
# paste cargo run output snippet here
```
