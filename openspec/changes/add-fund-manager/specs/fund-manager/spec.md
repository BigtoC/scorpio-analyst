## ADDED Requirements

### Requirement: Fund Manager Final Decision Agent

The system MUST implement a Fund Manager agent in `src/agents/fund_manager.rs` as a `rig` agent
using the `DeepThinking` model tier. The agent MUST be configured with the system prompt defined in
`docs/prompts.md` section 5 (Fund Manager). The agent reads `TradingState::trader_proposal`, the
three `RiskReport` fields (`aggressive_risk_report`, `neutral_risk_report`,
`conservative_risk_report`), `risk_discussion_history`, and the existing analyst context
(`fundamental_metrics`, `technical_indicators`, `market_sentiment`, `macro_news`), then writes a
validated `ExecutionStatus` to `TradingState::final_execution_status`.

#### Scenario: Approval with all inputs present

- **WHEN** the Fund Manager is invoked with a populated `TradeProposal`, three populated
  `RiskReport` objects (none with `flags_violation == true`), and a non-empty
  `risk_discussion_history`
- **THEN** the agent calls the `DeepThinking` LLM, produces a valid `ExecutionStatus` JSON with
  `decision = Approved`, a non-empty `rationale`, and `decided_at` normalized to the
  runtime-authoritative decision timestamp (falling back to the analysis date when a more precise
  timestamp is unavailable), and writes it to `TradingState::final_execution_status`

#### Scenario: LLM-based rejection when risk evidence warrants it

- **WHEN** the Fund Manager is invoked with risk reports where only the Conservative agent flags a
  violation (`flags_violation == true`) but the Neutral agent does not
- **THEN** the agent calls the `DeepThinking` LLM, and the LLM MAY return either `Approved` or
  `Rejected` based on the full evidence; the deterministic fallback does NOT trigger

#### Scenario: Missing trade proposal

- **WHEN** the Fund Manager is invoked and `TradingState::trader_proposal` is `None`
- **THEN** the agent returns a `TradingError` immediately without invoking the LLM, and
  `TradingState::final_execution_status` remains `None`

#### Scenario: Missing risk reports with partial data

- **WHEN** the Fund Manager is invoked with a populated `TradeProposal` but one or more
  `RiskReport` fields are `None` (e.g., a risk agent failed during Phase 4)
- **THEN** the agent acknowledges the missing risk data in the prompt context and still invokes the
  LLM to render a decision, noting the data gap in `rationale`

#### Scenario: Missing analyst context with partial upstream data

- **WHEN** the Fund Manager is invoked with a populated `TradeProposal` and risk inputs but one or
  more analyst fields (`fundamental_metrics`, `technical_indicators`, `market_sentiment`,
  `macro_news`) are `None`
- **THEN** the agent still invokes the LLM using the available context and the resulting
  `ExecutionStatus.rationale` acknowledges the missing upstream analyst data rather than implying a
  fully informed decision

### Requirement: Deterministic Safety-Net Rejection

The system MUST implement a deterministic rejection rule that bypasses the LLM entirely: if BOTH
`TradingState::conservative_risk_report` and `TradingState::neutral_risk_report` have
`flags_violation == true`, the Fund Manager MUST return `Decision::Rejected` with a rationale
stating the dual-violation condition, without invoking the LLM.

#### Scenario: Both Conservative and Neutral flag violation

- **WHEN** the Fund Manager is invoked and both `conservative_risk_report.flags_violation` and
  `neutral_risk_report.flags_violation` are `true`
- **THEN** the agent writes `ExecutionStatus { decision: Rejected, rationale: <dual-violation
  explanation>, decided_at: <runtime-authoritative timestamp or analysis-date fallback> }` to
  `TradingState::final_execution_status` without making any LLM call

#### Scenario: Only one of Conservative or Neutral flags violation

- **WHEN** exactly one of `conservative_risk_report.flags_violation` or
  `neutral_risk_report.flags_violation` is `true`
- **THEN** the deterministic rejection rule does NOT trigger and the agent proceeds to the LLM path

#### Scenario: Deterministic rejection token usage

- **WHEN** the deterministic rejection path is taken
- **THEN** the returned `AgentTokenUsage` has `token_counts_available = false`,
  `prompt_tokens = 0`, `completion_tokens = 0`, `total_tokens = 0`, and measured wall-clock
  `latency_ms`

### Requirement: ExecutionStatus Schema Validation

The system MUST validate the LLM's JSON response against the `ExecutionStatus` schema before
writing to `TradingState::final_execution_status`. Validation failures MUST return
`TradingError::SchemaViolation` and MUST NOT write to state.

#### Scenario: Valid Approved response

- **WHEN** the LLM returns a JSON response with `decision = "Approved"` and a non-empty `rationale`
- **THEN** the response is deserialized into an `ExecutionStatus` with `Decision::Approved`, the
  runtime overwrites `decided_at` with the authoritative decision timestamp it selected, and it is written to
  `TradingState::final_execution_status`

#### Scenario: Valid Rejected response

- **WHEN** the LLM returns a JSON response with `decision = "Rejected"` and a non-empty `rationale`
- **THEN** the response is deserialized into an `ExecutionStatus` with `Decision::Rejected`, the
  runtime overwrites `decided_at` with the authoritative decision timestamp it selected, and it is written to
  `TradingState::final_execution_status`

#### Scenario: Missing decided_at field from LLM response

- **WHEN** the LLM returns otherwise valid `ExecutionStatus` JSON but omits `decided_at` or
  provides a stale value
- **THEN** the runtime fills or overwrites `decided_at` with the authoritative decision timestamp
  it selected before writing to `TradingState::final_execution_status`

#### Scenario: Empty rationale

- **WHEN** the LLM returns a JSON response with an empty `rationale` string
- **THEN** the agent returns `TradingError::SchemaViolation` and
  `TradingState::final_execution_status` remains `None`

#### Scenario: Invalid decision value

- **WHEN** the LLM returns a JSON response with `decision` set to a value other than `"Approved"`
  or `"Rejected"` (e.g., `"Maybe"`, `"Hold"`)
- **THEN** the agent returns `TradingError::SchemaViolation` and
  `TradingState::final_execution_status` remains `None`

#### Scenario: Unparseable JSON response

- **WHEN** the LLM returns a response that does not parse as valid `ExecutionStatus` JSON
- **THEN** the agent returns `TradingError::SchemaViolation` and
  `TradingState::final_execution_status` remains `None`

#### Scenario: Rationale exceeds length bound or contains control characters

- **WHEN** the LLM returns a `rationale` that exceeds the module's documented length bound or
  contains disallowed control characters (any control character other than `\n` and `\t`)
- **THEN** the agent returns `TradingError::SchemaViolation` and
  `TradingState::final_execution_status` remains `None`

### Requirement: Fund Manager Token Usage Tracking

The Fund Manager invocation MUST record an `AgentTokenUsage` entry immediately after the LLM
completion call returns (or immediately after the deterministic bypass completes). The entry MUST
be returned to the caller for incorporation into the phase-level `PhaseTokenUsage`.

#### Scenario: LLM path with authoritative token counts

- **WHEN** the Fund Manager completes successfully using a provider that reports authoritative
  token counts (e.g., OpenAI)
- **THEN** the returned `AgentTokenUsage` contains agent name "Fund Manager", the correct model ID,
  `token_counts_available = true`, accurate prompt/completion/total token counts, and measured
  wall-clock latency

#### Scenario: LLM path without authoritative token counts

- **WHEN** the Fund Manager completes successfully using a provider that does not report
  authoritative token counts (e.g., Copilot via ACP)
- **THEN** the returned `AgentTokenUsage` contains agent name "Fund Manager", the correct model ID,
  `token_counts_available = false`, and measured wall-clock latency, with token count fields using
  the documented unavailable representation

### Requirement: Fund Manager Module Boundary

This capability's implementation MUST remain limited to Fund Manager agent concerns within
`src/agents/fund_manager.rs`. Tests MAY be colocated in the same file or placed in an adjacent
test-only module, but the production implementation for this capability MUST remain owned by
`src/agents/fund_manager.rs`. It MUST NOT modify any foundation-owned state types, provider code,
data layer files, or other agent modules.

#### Scenario: Downstream orchestration import

- **WHEN** the downstream `add-graph-orchestration` change imports the Fund Manager
- **THEN** it uses `use scorpio_analyst::agents::fund_manager::{run_fund_manager, FundManagerAgent}`
  and receives the entry point function and agent type through the agent module path

#### Scenario: No foreign file modifications beyond approved cross-owner change

- **WHEN** the Fund Manager module is implemented
- **THEN** the foundation-owned `Cargo.toml`, `src/lib.rs`, `src/state/*`, `src/config.rs`,
  `src/error.rs`, `src/rate_limit.rs`, the provider-owned `src/providers/*`, the data-layer
  `src/data/*`, the indicator `src/indicators/*`, the analyst-owned `src/agents/analyst/*`, the
  researcher-owned `src/agents/researcher/*`, the trader-owned `src/agents/trader/*`, and the
  risk-owned `src/agents/risk/*` files all remain unmodified, and the only cross-owner change is
  the approved `pub mod fund_manager;` uncomment in `src/agents/mod.rs`
