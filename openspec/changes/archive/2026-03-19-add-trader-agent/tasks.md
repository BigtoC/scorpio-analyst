# Tasks for `add-trader-agent`

## 0. Prerequisites

- [x] `add-project-foundation` is complete (core types including `TradeProposal`, `TradeAction`, `TradingState`,
      `AgentTokenUsage`, error handling, config)
- [x] `add-llm-providers` is complete (provider factory, agent builder helper, `DeepThinking` tier,
      `prompt_typed_with_retry` helper with structured output extraction and usage metadata)

## 1. Trader Agent (`src/agents/trader/mod.rs`)

- [x] 1.1 Define the Trader Agent system prompt as a `const &str`, adapted from `docs/prompts.md`
      (Trader section), with placeholders for `{ticker}`, `{current_date}`, `{consensus_summary}`,
      `{fundamental_report}`, `{technical_report}`, `{sentiment_report}`, `{news_report}`, and
      `{past_memory_str}`
- [x] 1.2 Implement `TraderAgent` struct with a constructor that accepts provider factory references
      and runtime parameters (asset symbol, target date)
- [x] 1.3 Implement context serialization helper that formats all `TradingState` fields into the prompt
      placeholders — missing analyst outputs serialized as `"null"`, missing `consensus_summary` serialized
      as an explicit absence note, and optional bounded debate-history context kept private to the module if used
- [x] 1.4 Implement `run(&self, state: &mut TradingState, config: &Config)
      -> Result<AgentTokenUsage, TradingError>` that builds a one-shot `rig` agent via the agent builder
      helper, invokes `prompt_typed_with_retry` to extract a typed `TradeProposal`, validates the
      result, writes to `state.trader_proposal`, and records `AgentTokenUsage`
- [x] 1.4a Ensure the Trader prompt instructs the model to align with the moderator's stance unless analyst
       evidence clearly justifies a different conclusion, and to explain any divergence in `rationale`
- [x] 1.5 Implement post-parse validation: `target_price > 0.0` and finite, `stop_loss > 0.0` and finite,
      `confidence` finite, `rationale` non-empty and within length bound, no disallowed control characters
      in `rationale`; `Hold` still requires numeric monitoring levels for `target_price` and `stop_loss`;
      return `TradingError::SchemaViolation` on failure
- [x] 1.6 Record `AgentTokenUsage` with agent name "Trader Agent", model ID from provider, token counts
      when available (respecting `token_counts_available` flag), and wall-clock latency

## 2. Public API (`src/agents/trader/mod.rs` exports)

- [x] 2.1 Expose `run_trader` function as the primary entry point that constructs a `TraderAgent` and
      invokes it, returning `Result<AgentTokenUsage, TradingError>`
- [x] 2.2 Re-export `TraderAgent` and `run_trader` for consumption by the downstream
      `add-graph-orchestration` change

## 3. Unit Tests

- [x] 3.1 Write unit test with mocked LLM response verifying a valid `TradeProposal` JSON is correctly
      deserialized and written to `state.trader_proposal`
- [x] 3.2 Write unit test verifying `action = Buy` produces `TradeAction::Buy`, and similarly for `Sell`
      and `Hold`
- [x] 3.2a Write unit test verifying a `Hold` proposal still requires numeric `target_price` and `stop_loss`
       values and preserves them as monitoring levels in the validated `TradeProposal`
- [x] 3.3 Write unit test verifying `target_price <= 0.0` is rejected with `TradingError::SchemaViolation`
- [x] 3.4 Write unit test verifying `stop_loss <= 0.0` is rejected with `TradingError::SchemaViolation`
- [x] 3.5 Write unit test verifying non-finite `confidence` (NaN, Infinity) is rejected with
      `TradingError::SchemaViolation`
- [x] 3.6 Write unit test verifying empty `rationale` is rejected with `TradingError::SchemaViolation`
- [x] 3.7 Write unit test verifying oversized or control-character-containing `rationale` is rejected with
      `TradingError::SchemaViolation`
- [x] 3.8 Write unit test verifying malformed or schema-invalid structured output is rejected with
      `TradingError::SchemaViolation` without typed-prompt retries on the same schema failure class
- [x] 3.9 Write unit test verifying `AgentTokenUsage` is recorded with agent name "Trader Agent" and
      correct model ID
- [x] 3.10 Write unit test verifying `AgentTokenUsage` preserves `token_counts_available = false` when
       the provider does not expose authoritative counts
- [x] 3.11 Write prompt-construction test verifying the moderator-alignment instruction and divergence-explanation
       instruction are present in the Trader prompt sent to the provider layer

## 4. Context Injection Tests

- [x] 4.1 Write unit test verifying all four analyst outputs are serialized into the prompt when present
- [x] 4.2 Write unit test verifying missing analyst outputs (graceful degradation) are serialized as
      `"null"` in the prompt context
- [x] 4.3 Write unit test verifying missing `consensus_summary` is handled with an explicit absence note
      in the prompt rather than an empty string or panic
- [x] 4.4 Write unit test verifying `{ticker}` and `{current_date}` are correctly substituted from
      `TradingState`

## 5. Approved Cross-Owner Touch-point (`src/agents/mod.rs`)

- [x] 5.1 Uncomment `pub mod trader;` in `src/agents/mod.rs` (line 8) so the trader module is compiled
      and accessible through the agent module path

## 6. Documentation and CI

- [x] 6.1 Add inline doc comments (`///`) for all public types and functions in `trader.rs`
- [x] 6.2 Ensure `cargo clippy -- -D warnings` passes with no new warnings
- [x] 6.3 Ensure `cargo fmt -- --check` passes
- [x] 6.4 Ensure `cargo test` passes all new and existing tests

### Cross-Owner Touch-points

- Approved for `chunk3-evidence-state-sync` to update `src/agents/trader/mod.rs` and `src/agents/trader/tests.rs`
  so the trader prompt includes the shared typed evidence/data-quality context and regression coverage pins the new
  prompt contract.
