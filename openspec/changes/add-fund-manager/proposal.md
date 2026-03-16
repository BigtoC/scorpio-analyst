# Change: Add Fund Manager Agent (Phase 5 — Final Execution Decision)

## Why

The 5-phase trading pipeline currently ends after the Risk Management Team (Phase 4). The Fund
Manager is the terminal decision-maker that reviews the trade proposal, all three risk
assessments, the full risk discussion history, and the supporting analyst context, then renders an
auditable approve/reject verdict. Without this agent the pipeline cannot produce a final
`ExecutionStatus`, blocking both the `add-graph-orchestration` and `add-cli` changes.

## What Changes

- Implement the Fund Manager agent in `src/agents/fund_manager.rs` as a `rig` agent on the
  `DeepThinking` model tier.
- Embed the system prompt from `docs/prompts.md` section 5 (Fund Manager).
- Provide a `run_fund_manager(state, config)` public entry point mirroring the pattern established
  by `add-trader-agent`'s `run_trader`.
- Apply the deterministic safety-net rule: **automatically reject** if both the Conservative and
  Neutral `RiskReport` objects have `flags_violation == true`, bypassing the LLM entirely.
- Pass the existing analyst context (`fundamental_metrics`, `technical_indicators`,
  `market_sentiment`, `macro_news`) to the Fund Manager prompt when available, and require the
  agent to acknowledge any missing upstream context rather than fabricate certainty.
- When the LLM path is taken, validate the response against the `ExecutionStatus` schema
  (`decision`, `rationale`, `decided_at`) and enforce bounded-text / control-character policies
  consistent with other agents.
- Normalize `ExecutionStatus.decided_at` to the runtime-authoritative decision timestamp, falling
  back to the current analysis date when a more precise timestamp is unavailable.
- Record an `AgentTokenUsage` entry for the single LLM invocation (or mark the deterministic
  bypass with zero token counts and measured latency).
- Write the validated `ExecutionStatus` to `TradingState::final_execution_status`.
- Reuse existing state and provider abstractions without changing `src/state/trading_state.rs`,
  `src/state/risk.rs`, `src/state/execution.rs`, or `src/providers/factory.rs`.

## Impact

- Affected specs: `fund-manager` (new capability)
- Affected code: `src/agents/fund_manager.rs` (new file, owned by this change); read-only
  dependencies on `src/state/trading_state.rs`, `src/state/risk.rs`, `src/state/execution.rs`,
  and `src/providers/factory.rs`

## Cross-Owner Changes

| File                   | Owner                    | Justification                                                                                                                                              |
|------------------------|--------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `src/agents/mod.rs:10` | `add-project-foundation` | Uncomment `pub mod fund_manager;` to wire the new module into the agent tree. Identical pattern approved for `add-trader-agent` and `add-risk-management`. |
