# Tasks for `add-researcher-debate`

## Prerequisites

- [ ] `add-project-foundation` is complete (core types including `DebateMessage`, `TradingState`, error handling,
      config with `max_debate_rounds`)
- [ ] `add-llm-providers` is complete (provider factory, agent builder helper, `DeepThinking` tier,
      `chat_with_retry` and `prompt_with_retry` helpers)
- [ ] Approved cross-owner provider touch-point is available in `src/providers/factory.rs` so retry-wrapped chat calls
      can also return usage metadata for researcher token accounting

## 1. Bullish Researcher Agent (`src/agents/researcher/bullish.rs`)

- [ ] 1.1 Define the Bullish Researcher system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Bull Researcher section), with placeholders for `{current_date}`, `{ticker}`, `{fundamental_report}`,
      `{technical_report}`, `{sentiment_report}`, `{news_report}`, `{debate_history}`,
      `{current_bear_argument}`, and `{past_memory_str}`
- [ ] 1.2 Implement `BullishResearcher` struct with a constructor that accepts provider factory references
      and runtime parameters (asset symbol, target date, serialized analyst data snapshots)
- [ ] 1.3 Implement `run(&mut self, debate_history: &[DebateMessage], bear_argument: Option<&str>)
      -> Result<(DebateMessage, AgentTokenUsage), TradingError>` that builds/continues a `rig` chat session
      via the retry-wrapped chat-details helper, extracts the plain-text response as
      `DebateMessage { role: "bullish_researcher", content }`, and records `AgentTokenUsage`
- [ ] 1.4 Write unit tests with mocked LLM responses verifying correct `DebateMessage` construction with
      `role = "bullish_researcher"`
- [ ] 1.5 Write unit tests verifying chat history accumulates across multiple `run` invocations
- [ ] 1.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Bullish Researcher"
      and correct model ID
- [ ] 1.7 Write unit tests verifying oversized or control-character-containing bullish outputs are rejected with
      `TradingError::SchemaViolation`

## 2. Bearish Researcher Agent (`src/agents/researcher/bearish.rs`)

- [ ] 2.1 Define the Bearish Researcher system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Bear Researcher section), with placeholders for runtime parameters and `{current_bull_argument}`
- [ ] 2.2 Implement `BearishResearcher` struct with a constructor that accepts provider factory references
      and runtime parameters (asset symbol, target date, serialized analyst data snapshots)
- [ ] 2.3 Implement `run(&mut self, debate_history: &[DebateMessage], bull_argument: Option<&str>)
      -> Result<(DebateMessage, AgentTokenUsage), TradingError>` that builds/continues a `rig` chat session
      via the retry-wrapped chat-details helper, extracts the plain-text response as
      `DebateMessage { role: "bearish_researcher", content }`, and records `AgentTokenUsage`
- [ ] 2.4 Write unit tests with mocked LLM responses verifying correct `DebateMessage` construction with
      `role = "bearish_researcher"`
- [ ] 2.5 Write unit tests verifying chat history accumulates across multiple `run` invocations
- [ ] 2.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Bearish Researcher"
- [ ] 2.7 Write unit tests verifying oversized or control-character-containing bearish outputs are rejected with
      `TradingError::SchemaViolation`

## 3. Debate Moderator Agent (`src/agents/researcher/moderator.rs`)

- [ ] 3.1 Define the Debate Moderator system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Debate Moderator section), with placeholders for `{bull_case}`, `{bear_case}`, `{debate_history}`,
      analyst data placeholders, and `{past_memory_str}`
- [ ] 3.2 Implement `DebateModerator` struct with a constructor that accepts provider factory references
      and runtime parameters
- [ ] 3.3 Implement `run(&self, state: &TradingState) -> Result<(String, AgentTokenUsage), TradingError>`
      that constructs a one-shot `rig` agent via `prompt_with_retry`, extracts the plain-text consensus
      summary, and records `AgentTokenUsage`
- [ ] 3.4 Write unit tests verifying the moderator produces a non-empty consensus summary
- [ ] 3.5 Write unit tests verifying the moderator's output includes an explicit stance (`Buy`, `Sell`, or
      `Hold`) as required by the prompt specification
- [ ] 3.6 Write unit tests verifying `AgentTokenUsage` is recorded with agent name "Debate Moderator"
- [ ] 3.7 Write unit tests verifying oversized or control-character-containing consensus summaries are rejected with
      `TradingError::SchemaViolation`

## 4. Cyclic Debate Loop (`src/agents/researcher/mod.rs`)

- [ ] 4.1 Implement `run_researcher_debate` function that accepts `&mut TradingState`, `&Config`, and
      provider references, and orchestrates the cyclic loop for `config.llm.max_debate_rounds` iterations
- [ ] 4.2 Within each round: invoke Bullish Researcher then Bearish Researcher sequentially, append each
      `DebateMessage` to `state.debate_history`, and collect `AgentTokenUsage` entries
- [ ] 4.3 After all rounds: invoke the Debate Moderator, write the consensus summary to
      `state.consensus_summary`, and collect the moderator's `AgentTokenUsage`
- [ ] 4.4 Return `Result<Vec<AgentTokenUsage>, TradingError>` containing all token usage entries from all
      rounds plus the moderator
- [ ] 4.5 Re-export `run_researcher_debate` and individual researcher types from
      `src/agents/researcher/mod.rs`
- [ ] 4.6 Write unit test: 1 round debate completes, verify 2 `DebateMessage` entries in `debate_history`
      and a populated `consensus_summary`
- [ ] 4.7 Write unit test: 3 round debate (default), verify 6 `DebateMessage` entries in `debate_history`
      (2 per round) and a populated `consensus_summary`
- [ ] 4.8 Write unit test: `max_debate_rounds = 0`, verify no debate messages, moderator still invoked
      to produce a consensus from analyst data alone
- [ ] 4.9 Write unit test: LLM failure on Bullish Researcher in round 2 propagates as
      `TradingError` and aborts the debate
- [ ] 4.10 Write unit test: verify `AgentTokenUsage` entries total `2 * max_debate_rounds + 1` (researchers + moderator)
- [ ] 4.11 Write unit test: verify researcher token entries preserve `token_counts_available = false` when the provider
       does not expose authoritative counts

## 5. Integration Tests

- [ ] 5.1 Write integration test: construct all three researcher agents with mocked provider, run
      `run_researcher_debate` with 2 rounds, verify `debate_history` has 4 entries and
      `consensus_summary` is populated
- [ ] 5.2 Write integration test: simulate partial analyst data (one `None` field on `TradingState`),
      verify debate completes without error and researchers acknowledge the gap
- [ ] 5.3 Write integration test: verify `AgentTokenUsage` entries are collected for all invocations
      including the moderator

## 5a. Approved Cross-Owner Provider Touch-point (`src/providers/factory.rs`)

- [ ] 5a.1 Add a minimal provider-layer surface for retry-wrapped chat usage details (for example,
      `chat_with_retry_details` and any supporting provider-agnostic `LlmAgent` chat-details method)
- [ ] 5a.2 Add provider-layer tests verifying the chat-details helper preserves the same retry/timeout/error-mapping
      semantics as `chat_with_retry` while also surfacing usage metadata when available

## 6. Documentation and CI

- [ ] 6.1 Add inline doc comments (`///`) for all public types and functions in `mod.rs`, `bullish.rs`,
      `bearish.rs`, and `moderator.rs`
- [ ] 6.1a If shared formatting or validation helpers are added, keep them in a private `src/agents/researcher/common.rs`
        module and do not re-export them publicly
- [ ] 6.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [ ] 6.3 Ensure `cargo fmt -- --check` passes
- [ ] 6.4 Ensure `cargo test` passes all new and existing tests
