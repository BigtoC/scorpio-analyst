# Tasks for `add-risk-management`

## Prerequisites

- [ ] `add-project-foundation` is complete (core types including `RiskReport`, `RiskLevel`, `DebateMessage`,
      `TradingState`, error handling, config with `max_risk_rounds`)
- [ ] `add-llm-providers` is complete (provider factory, agent builder helper, `DeepThinking` tier,
      `prompt_with_retry_details` and `chat_with_retry_details` helpers)

## 1. Aggressive Risk Agent (`src/agents/risk/aggressive.rs`)

- [ ] 1.1 Define the Aggressive Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Aggressive Risk Analyst section), with placeholders for `{current_date}`, `{ticker}`, `{trader_proposal}`,
      `{fundamental_report}`, `{technical_report}`, `{sentiment_report}`, `{news_report}`, `{risk_history}`,
      `{conservative_response}`, `{neutral_response}`, and `{past_memory_str}`
- [ ] 1.2 Implement `AggressiveRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters (asset symbol, target date, serialized analyst data, trade proposal)
- [ ] 1.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `prompt_with_retry_details` with structured `RiskReport` extraction,
      validates that the returned `risk_level == Aggressive`, and records `AgentTokenUsage`
- [ ] 1.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Aggressive`
- [ ] 1.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [ ] 1.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Aggressive Risk Analyst"
      and correct model ID
- [ ] 1.7 Write unit tests verifying that missing `trader_proposal` produces an appropriate error

## 2. Conservative Risk Agent (`src/agents/risk/conservative.rs`)

- [ ] 2.1 Define the Conservative Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Conservative Risk Analyst section), with placeholders for runtime parameters, `{aggressive_response}`,
      and `{neutral_response}`
- [ ] 2.2 Implement `ConservativeRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters
- [ ] 2.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `prompt_with_retry_details` with structured `RiskReport` extraction,
      validates that the returned `risk_level == Conservative`, and records `AgentTokenUsage`
- [ ] 2.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Conservative`
- [ ] 2.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [ ] 2.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Conservative Risk Analyst"

## 3. Neutral Risk Agent (`src/agents/risk/neutral.rs`)

- [ ] 3.1 Define the Neutral Risk Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Neutral Risk Analyst section), with placeholders for runtime parameters, `{aggressive_response}`,
      and `{conservative_response}`
- [ ] 3.2 Implement `NeutralRiskAgent` struct with a constructor that accepts provider factory references
      and runtime parameters
- [ ] 3.3 Implement `run(&mut self, state: &TradingState) -> Result<(RiskReport, AgentTokenUsage), TradingError>`
      that constructs a `rig` agent via `prompt_with_retry_details` with structured `RiskReport` extraction,
      validates that the returned `risk_level == Neutral`, and records `AgentTokenUsage`
- [ ] 3.4 Write unit tests with mocked LLM responses verifying correct `RiskReport` construction with
      `risk_level = Neutral`
- [ ] 3.5 Write unit tests verifying `RiskReport` with wrong `risk_level` is rejected with
      `TradingError::SchemaViolation`
- [ ] 3.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Neutral Risk Analyst"

## 4. Risk Moderator Agent (`src/agents/risk/moderator.rs`)

- [ ] 4.1 Define the Risk Moderator system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Risk Moderator section), with placeholders for `{aggressive_case}`, `{neutral_case}`,
      `{conservative_case}`, `{risk_history}`, analyst data placeholders, and `{past_memory_str}`
- [ ] 4.2 Implement `RiskModerator` struct with a constructor that accepts provider factory references
      and runtime parameters
- [ ] 4.3 Implement `run(&self, state: &TradingState) -> Result<(String, AgentTokenUsage), TradingError>`
      that constructs a one-shot `rig` agent via `prompt_with_retry_details`, extracts the plain-text synthesis,
      and records `AgentTokenUsage`
- [ ] 4.4 Write unit tests verifying the moderator produces a non-empty discussion synthesis
- [ ] 4.5 Write unit tests verifying the moderator's output explicitly references whether Conservative and
      Neutral both flag a violation
- [ ] 4.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Risk Moderator"
- [ ] 4.7 Write unit tests verifying oversized or control-character-containing moderator outputs are rejected with
      `TradingError::SchemaViolation`

## 5. Cyclic Risk Discussion Loop (`src/agents/risk/mod.rs`)

- [ ] 5.1 Implement `run_risk_discussion` function that accepts `&mut TradingState`, `&Config`, and
      provider references, and orchestrates the cyclic loop for `config.llm.max_risk_rounds` iterations
- [ ] 5.2 Validate that `state.trader_proposal` is `Some` before starting the discussion, returning
      `TradingError` if missing
- [ ] 5.3 Within each round: invoke Aggressive, Conservative, and Neutral risk agents sequentially; write each
      `RiskReport` to the corresponding `TradingState` field and append a `DebateMessage` summary to
      `state.risk_discussion_history`; collect `AgentTokenUsage` entries
- [ ] 5.4 After all rounds: invoke the Risk Moderator, append the synthesis to `state.risk_discussion_history`,
      and collect the moderator's `AgentTokenUsage`
- [ ] 5.5 Return `Result<Vec<AgentTokenUsage>, TradingError>` containing all token usage entries from all
      rounds plus the moderator
- [ ] 5.6 Re-export `run_risk_discussion` and individual risk agent types from `src/agents/risk/mod.rs`
- [ ] 5.7 Write unit test: 1 round discussion completes, verify 3 `RiskReport` fields populated in
      `TradingState` and risk discussion history contains entries
- [ ] 5.8 Write unit test: 2 round discussion (default), verify 6 risk persona `DebateMessage` entries + 1
      moderator entry in `risk_discussion_history` and all 3 `RiskReport` fields populated
- [ ] 5.9 Write unit test: `max_risk_rounds = 0`, verify no risk persona messages, moderator still invoked
      to produce a synthesis from the trade proposal alone
- [ ] 5.10 Write unit test: risk agent failure in round 2 propagates as `TradingError` and aborts the discussion
- [ ] 5.11 Write unit test: missing `trader_proposal` returns appropriate error before any LLM calls
- [ ] 5.12 Write unit test: verify `AgentTokenUsage` entries total `3 * max_risk_rounds + 1`
      (3 persona agents per round + 1 moderator)
- [ ] 5.13 Write unit test: verify risk agent token entries preserve `token_counts_available = false` when the
      provider does not expose authoritative counts

## 6. End-to-End Risk Discussion Tests

- [ ] 6.1 Write end-to-end orchestration test (via a test-only risk discussion executor seam) that runs
      `run_risk_discussion` logic for multiple rounds and verifies `risk_discussion_history` entry count,
      role correctness, and populated `RiskReport` fields
- [ ] 6.2 Write mocked-provider risk tests covering partial analyst data serialization as `null`,
      `TradeProposal` context injection, and repeated `run` invocations with accumulated discussion history
- [ ] 6.3 Write end-to-end token usage tests verifying per-invocation `AgentTokenUsage` collection order
      and moderator inclusion
- [ ] 6.4 Write test verifying that `RiskReport.flags_violation` values are correctly preserved through the
      discussion loop for downstream Fund Manager consumption

## 7. Documentation and CI

- [ ] 7.1 Add inline doc comments (`///`) for all public types and functions in `mod.rs`, `aggressive.rs`,
      `conservative.rs`, `neutral.rs`, and `moderator.rs`
- [ ] 7.1a If shared formatting or validation helpers are added, keep them in a private `src/agents/risk/common.rs`
         module and do not re-export them publicly
- [ ] 7.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [ ] 7.3 Ensure `cargo fmt -- --check` passes
- [ ] 7.4 Ensure `cargo test` passes all new and existing tests
