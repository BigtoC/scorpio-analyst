# `core-types` Capability

## ADDED Requirements

### Requirement: Core Trading State Schema

The system MUST define a `TradingState` struct that models the unified shared memory object propagated through the
workflow. It MUST include stable fields for run identity (`execution_id`, `asset_symbol`, `target_date`), optional
phase outputs for analyst, trader, risk, and final manager stages, debate and risk discussion history, and aggregated
`TokenUsageTracker` accounting.

#### Scenario: Workflow Initialization

When a trade generation signal fires, the system initializes an empty `TradingState` containing a unique
`execution_id`, target `asset_symbol`, and `target_date`, populating optional phase outputs as `Option::None`,
initializing debate and risk history collections as empty, and creating an empty `TokenUsageTracker`.

### Requirement: Data Sub-Struct Enumeration

The system MUST define structs explicitly modeling the data boundaries of analyst outputs.

- `FundamentalData` (e.g., revenue growth, P/E, liquidity, insider transactions).
- `TechnicalData` (e.g., RSI, MACD, ATR, support/resistance levels).
- `SentimentData` (e.g., normalized scores, source breakdown, engagement peaks).
- `NewsData` (e.g., articles, macro events, causal relationships).

#### Scenario: Storing Analyst Output

When the Fundamental Analyst completes successfully, its output is strictly serialized and populated into
`TradingState::fundamental_metrics` mapping precisely to `FundamentalData`.

### Requirement: Debate And Risk Discussion Constructs

The system MUST define stable structures for debate and risk-audit state so downstream researcher, trader, risk, and
manager capabilities reuse the same data model.

- `DebateRound` or an equivalent typed representation for debate history entries
- `ConsensusResult` or an equivalent typed representation for the debate outcome
- `TradeProposal` (action: Buy/Sell/Hold, target price, stop-loss, confidence)
- `RiskReport` (assessment, risk level, recommended adjustments)
- `ExecutionStatus` (approved/rejected, rationale, timestamps)
- dedicated `TradingState` slots for `debate_history`, `consensus_summary` or equivalent consensus result,
  `risk_discussion_history`, `aggressive_risk_report`, `neutral_risk_report`, `conservative_risk_report`, and
  `final_execution_status`

#### Scenario: Resolving Trader Output

When the Trader task issues a Buy action after researcher debate concludes, the generated payload binds to
`TradeProposal`, references the stored consensus result, and guarantees target price and stop loss exist.

### Requirement: Token Usage Tracking

The system MUST measure token expenditures accurately using `TokenUsageTracker`, `PhaseTokenUsage`, and
`AgentTokenUsage`.

- `TokenUsageTracker` MUST capture aggregate prompt, completion, and total token counts for the entire run.
- `PhaseTokenUsage` MUST capture a phase name, duration, aggregate phase totals, and nested per-agent entries.
- `AgentTokenUsage` MUST capture agent name, model ID, prompt/completion/total tokens, and latency.
- Cyclic phases MUST support multiple `PhaseTokenUsage` entries so individual debate and risk rounds can be tracked
  separately.

#### Scenario: Aggregating End of Cycle Tokens

At workflow conclusion, the orchestrator retrieves token metadata collected across all tasks, including per-phase and
per-agent totals, latency, and cyclic round breakdowns stored via `TokenUsageTracker`.

### Requirement: Foundation Serialization Contract

All foundational state and domain structs owned by `core-types` MUST support serialization and deserialization for JSON
snapshotting, test round-trips, and downstream reuse.

#### Scenario: Persisting A Trading Snapshot

When the application serializes a completed `TradingState` to JSON for an audit snapshot, the full state including token
usage, debate history, and risk reports round-trips back into the same typed structures without losing required fields.
