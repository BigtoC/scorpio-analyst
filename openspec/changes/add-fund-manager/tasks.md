## 1. Cross-Owner Setup

- [x] 1.1 Obtain approval for the cross-owner edit to uncomment `pub mod fund_manager;` in `src/agents/mod.rs:10`
- [x] 1.2 Uncomment `pub mod fund_manager;` in `src/agents/mod.rs:10`

## 2. Core Implementation

- [x] 2.1 Create `src/agents/fund_manager.rs`
- [x] 2.2 Embed `FUND_MANAGER_SYSTEM_PROMPT` const from `docs/prompts.md` section 5
- [x] 2.3 Implement `FundManagerInference` trait (mirrors the existing trait-based agent test seam pattern)
- [x] 2.4 Implement `FundManagerAgent` struct with `new(handle, symbol, target_date, llm_config)` constructor
- [x] 2.5 Implement deterministic safety-net check: reject if both Conservative + Neutral `flags_violation == true`
- [x] 2.6 Implement `build_prompt_context` to inject serialized `TradeProposal`, 3 `RiskReport` objects, `risk_discussion_history`, analyst data, and `{current_date}`
- [x] 2.7 Implement prompt sanitization (reuse or mirror `sanitize_prompt_context`, `sanitize_symbol_for_prompt`, `sanitize_date_for_prompt`, `redact_secret_like_values` from trader module as local logic, without cross-owner edits)
- [x] 2.8 Implement `run` method: deterministic check -> LLM call -> validate -> write to `final_execution_status`
- [x] 2.9 Implement `validate_execution_status`: valid decision enum, non-empty rationale, bounded length, no disallowed control chars
- [x] 2.10 Normalize `decided_at` to the runtime-authoritative decision timestamp, falling back to the analysis date when a more precise timestamp is unavailable
- [x] 2.11 Implement `usage_from_response` for `AgentTokenUsage` construction (LLM path and deterministic bypass)

## 3. Public Entry Point

- [x] 3.1 Implement `run_fund_manager(state, config) -> Result<AgentTokenUsage, TradingError>` public function
- [x] 3.2 Verify `run_fund_manager` creates `DeepThinking` handle via `create_completion_model`
- [x] 3.3 Export `run_fund_manager` and `FundManagerAgent` from the module

## 4. Testing

- [x] 4.1 Add Fund Manager tests in a test-only module for `src/agents/fund_manager.rs`
- [x] 4.2 Test: deterministic rejection when both Conservative + Neutral flag violation
- [x] 4.3 Test: LLM path taken when only Conservative flags violation (Neutral does not)
- [x] 4.4 Test: LLM path taken when neither flags violation
- [x] 4.5 Test: error returned when `trader_proposal` is `None`
- [x] 4.6 Test: valid Approved `ExecutionStatus` written to state
- [x] 4.7 Test: valid Rejected `ExecutionStatus` written to state
- [x] 4.8 Test: `SchemaViolation` on empty rationale
- [x] 4.9 Test: `SchemaViolation` on invalid decision value
- [x] 4.10 Test: `SchemaViolation` on rationale with disallowed control characters
- [x] 4.11 Test: `SchemaViolation` on rationale exceeding length bound
- [x] 4.12 Test: `decided_at` normalized to runtime timestamp or analysis-date fallback
- [x] 4.13 Test: `AgentTokenUsage` populated correctly for LLM path
- [x] 4.14 Test: `AgentTokenUsage` populated correctly for deterministic bypass (zero tokens, measured latency)
- [x] 4.15 Test: missing risk reports still invoke LLM with data-gap acknowledgment
- [x] 4.16 Test: missing analyst inputs still invoke LLM with data-gap acknowledgment

## 5. Verification

- [x] 5.1 Run `cargo fmt -- --check`
- [x] 5.2 Run `cargo clippy` (zero warnings)
- [x] 5.3 Run `cargo test` (all tests pass)
- [x] 5.4 Run `cargo build` (clean compile)

### Cross-Owner Touch-points

- Completed: `src/agents/mod.rs` (owned by `add-project-foundation`) — uncommented line 10 `pub mod fund_manager;`
