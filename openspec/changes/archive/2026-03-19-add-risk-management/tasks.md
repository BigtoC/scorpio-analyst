# Tasks for `add-risk-management`

## Prerequisites

- [x] `add-project-foundation` is complete (core types including `RiskReport`, `RiskLevel`, `DebateMessage`,
      `TradingState`, error handling, config with `max_risk_rounds`)
- [x] `add-llm-providers` is complete (provider factory, agent builder helper, `DeepThinking` tier,
      `prompt_with_retry_details` and `chat_with_retry_details` helpers)

## 1. Aggressive Risk Agent (`src/agents/risk/aggressive.rs`)

- [x] 1.1 Define the Aggressive Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Aggressive Risk Analyst section), with placeholders for `{current_date}`, `{ticker}`, `{trader_proposal}`,
      `{fundamental_report}`, `{technical_report}`, `{sentiment_report}`, `{news_report}`, `{risk_history}`,
      `{conservative_response}`, `{neutral_response}`, and `{past_memory_str}`
- [x] 1.2 Implement `AggressiveRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters (asset symbol, target date, serialized analyst data, trade proposal)
- [x] 1.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `chat_with_retry_details`, locally deserializes the raw JSON string into
      `RiskReport`, validates that the returned `risk_level == Aggressive`, and records `AgentTokenUsage`
- [x] 1.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Aggressive`
- [x] 1.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [x] 1.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Aggressive Risk Analyst"
      and correct model ID
- [x] 1.7 Write unit tests verifying that missing `trader_proposal` produces an appropriate error
- [x] 1.8 Write unit tests verifying prompt-bound context is sanitized, treats injected data as untrusted,
      redacts secret-like substrings, and bounds discussion-history growth
- [x] 1.9 Write unit tests verifying `assessment` and each `recommended_adjustments` entry reject disallowed
      control characters or oversized payloads with `TradingError::SchemaViolation`

## 2. Conservative Risk Agent (`src/agents/risk/conservative.rs`)

- [x] 2.1 Define the Conservative Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Conservative Risk Analyst section), with placeholders for runtime parameters, `{aggressive_response}`,
      and `{neutral_response}`
- [x] 2.2 Implement `ConservativeRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters
- [x] 2.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `chat_with_retry_details`, locally deserializes the raw JSON string into
      `RiskReport`, validates that the returned `risk_level == Conservative`, and records `AgentTokenUsage`
- [x] 2.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Conservative`
- [x] 2.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [x] 2.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Conservative Risk Analyst"
- [x] 2.7 Write unit tests verifying `assessment` and each `recommended_adjustments` entry reject disallowed
      control characters or oversized payloads with `TradingError::SchemaViolation`

## 3. Neutral Risk Agent (`src/agents/risk/neutral.rs`)

- [x] 3.1 Define the Neutral Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Neutral Risk Analyst section), with placeholders for runtime parameters, `{aggressive_response}`,
      and `{conservative_response}`
- [x] 3.2 Implement `NeutralRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters
- [x] 3.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `chat_with_retry_details`, locally deserializes the raw JSON string into
      `RiskReport`, validates that the returned `risk_level == Neutral`, and records `AgentTokenUsage`
- [x] 3.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Neutral`
- [x] 3.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [x] 3.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Neutral Risk Analyst"
- [x] 3.7 Write unit tests verifying `assessment` and each `recommended_adjustments` entry reject disallowed
      control characters or oversized payloads with `TradingError::SchemaViolation`

## 4. Risk Moderator Agent (`src/agents/risk/moderator.rs`)

- [x] 4.1 Define the Risk Moderator system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Risk Moderator section), with placeholders for `{aggressive_case}`, `{neutral_case}`,
      `{conservative_case}`, `{risk_history}`, analyst data placeholders, and `{past_memory_str}`
- [x] 4.2 Implement `RiskModerator` struct with a constructor that accepts provider factory references
      and runtime parameters
- [x] 4.3 Implement `run(&self, state: &TradingState) -> Result<(String, AgentTokenUsage), TradingError>`
      that constructs a one-shot `rig` agent via `prompt_with_retry_details`, extracts the plain-text synthesis,
      and records `AgentTokenUsage`
- [x] 4.4 Write unit tests verifying the moderator produces a non-empty discussion synthesis
- [x] 4.5 Write unit tests verifying the moderator's output explicitly references whether Conservative and
      Neutral both flag a violation
- [x] 4.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Risk Moderator"
- [x] 4.7 Write unit tests verifying oversized or control-character-containing moderator outputs are rejected with
      `TradingError::SchemaViolation`

## 5. Cyclic Risk Discussion Loop (`src/agents/risk/mod.rs`)

- [x] 5.1 Implement `run_risk_discussion` function that accepts `&mut TradingState`, `&Config`, and
      provider references, and orchestrates the cyclic loop for `config.llm.max_risk_rounds` iterations
- [x] 5.2 Validate that `state.trader_proposal` is `Some` before starting the discussion, returning
      `TradingError` if missing
- [x] 5.3 Within each round: invoke Aggressive, Conservative, and Neutral risk agents sequentially; write each
      `RiskReport` to the corresponding `TradingState` field and append a `DebateMessage` summary to
      `state.risk_discussion_history`; collect `AgentTokenUsage` entries
- [x] 5.4 After all rounds: invoke the Risk Moderator, append the synthesis to `state.risk_discussion_history`,
      and collect the moderator's `AgentTokenUsage`
- [x] 5.5 Return `Result<Vec<AgentTokenUsage>, TradingError>` containing all token usage entries from all
      rounds plus the moderator
- [x] 5.6 Re-export `run_risk_discussion` and individual risk agent types from `src/agents/risk/mod.rs`
- [x] 5.6a Document in the module-level API docs that persona turns are sequential within each round because prompts
         depend on the other agents' latest same-round views
- [x] 5.7 Write unit test: 1 round discussion completes, verify 3 `RiskReport` fields populated in
      `TradingState` and risk discussion history contains entries
- [x] 5.8 Write unit test: 2 round discussion (default), verify 6 risk persona `DebateMessage` entries + 1
      moderator entry in `risk_discussion_history` and all 3 `RiskReport` fields populated
- [x] 5.9 Write unit test: `max_risk_rounds = 0`, verify no risk persona messages, moderator still invoked
      to produce a synthesis from the trade proposal alone
- [x] 5.10 Write unit test: risk agent failure in round 2 propagates as `TradingError` and aborts the discussion
- [x] 5.11 Write unit test: missing `trader_proposal` returns appropriate error before any LLM calls
- [x] 5.12 Write unit test: verify `AgentTokenUsage` entries total `3 * max_risk_rounds + 1`
      (3 persona agents per round + 1 moderator)
- [x] 5.13 Write unit test: verify risk agent token entries preserve `token_counts_available = false` when the
      provider does not expose authoritative counts

## 6. End-to-End Risk Discussion Tests

- [x] 6.1 Write end-to-end orchestration test (via a test-only risk discussion executor seam) that runs
      `run_risk_discussion` logic for multiple rounds and verifies `risk_discussion_history` entry count,
      role correctness, and populated `RiskReport` fields
- [x] 6.2 Write mocked-provider risk tests covering partial analyst data serialization as `null`,
      `TradeProposal` context injection, injected-context sanitization/redaction, and repeated `run` invocations with
      accumulated discussion history
- [x] 6.3 Write end-to-end token usage tests verifying per-invocation `AgentTokenUsage` collection order
      and moderator inclusion
- [x] 6.4 Write test verifying that `RiskReport.flags_violation` values are correctly preserved through the
      discussion loop for downstream Fund Manager consumption

## 7. Documentation and CI

- [x] 7.1 Add inline doc comments (`///`) for all public types and functions in `mod.rs`, `aggressive.rs`,
      `conservative.rs`, `neutral.rs`, and `moderator.rs`
- [x] 7.1a If shared formatting or validation helpers are added, keep them in a private `src/agents/risk/common.rs`
         module and do not re-export them publicly
- [x] 7.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [x] 7.3 Ensure `cargo fmt -- --check` passes
- [x] 7.4 Ensure `cargo test` passes all new and existing tests

## 8. Post-Review Remediation

- [x] 8.1 Align persona-turn orchestration with the approved spec by passing serialized peer `RiskReport`
      context between rounds and restoring the required `DebateMessage.role` values
- [x] 8.2 Harden prompt/storage secret redaction and bound reinjected risk-history context to avoid
      unbounded prompt growth
- [x] 8.3 Enforce the Risk Moderator's required Conservative+Neutral violation-status sentence and
      sanitize prompt-bound symbol/date values
- [x] 8.4 Add regression tests covering same-round peer-view propagation, malformed persona JSON,
      oversized adjustments, repeated persona chat history, moderator failure propagation, and
      redaction-on-write behavior

### Cross-Owner Touch-points

- Approved for `chunk3-evidence-state-sync` to update `src/agents/risk/common.rs` so the shared risk analyst-context
  builder appends the typed evidence/data-quality context blocks while preserving the existing legacy analyst snapshot.
