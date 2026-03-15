# Change: Add Trader Agent

## Why

The Trader Agent is Phase 3 of the TradingAgents pipeline. Without it, the balanced `consensus_summary` produced by
the Researcher Debate has no consumer to synthesize it into a concrete, auditable trade directive. This proposal
introduces the Trader Agent — implemented as a single `rig` agent using the `DeepThinking` model tier — that reads
the full `TradingState` (all analyst data plus the debate consensus) and produces a structured `TradeProposal` JSON
object for downstream evaluation by the Risk Management Team.

## What Changes

- Implement `TraderAgent` in `src/agents/trader.rs` — uses `DeepThinking` tier, receives the full `TradingState`
  context (analyst outputs + consensus summary), and produces a structured `TradeProposal` JSON output via
  `prompt_with_retry` (one-shot, not chat-based).
- The system prompt is derived from `docs/prompts.md` (Trader section) with placeholders for `{ticker}`,
  `{current_date}`, `{consensus_summary}`, `{fundamental_report}`, `{technical_report}`, `{sentiment_report}`,
  `{news_report}`, and `{past_memory_str}`.
- The agent writes the validated `TradeProposal` to `TradingState::trader_proposal`.
- The agent records `AgentTokenUsage` (agent name "Trader Agent", model ID, prompt/completion/total tokens,
  wall-clock latency, and `token_counts_available` flag) for the `TokenUsageTracker`.
- The LLM response is parsed and validated against the `TradeProposal` schema: `action` must be a valid
  `TradeAction` variant, `target_price` and `stop_loss` must be finite positive numbers, `confidence` must be a
  finite number, and `rationale` must be non-empty and bounded.
- Expose `run_trader` function and `TraderAgent` type from `src/agents/trader.rs` for consumption by the
  downstream `add-graph-orchestration` change.

## Impact

- Affected specs: `trader-agent` (new)
- Affected code: `src/agents/trader.rs` (new file)
- Dependencies: `add-project-foundation` (core types including `TradeProposal`, `TradeAction`, `TradingState`,
  error handling, config), `add-llm-providers` (provider factory, agent builder helper, `DeepThinking` tier,
  `prompt_with_retry` helper)
- No modifications to foundation-owned files (`src/config.rs`, `src/error.rs`, `src/state/*`), provider-owned
  files (`src/providers/*`), data-layer files (`src/data/*`), indicator files (`src/indicators/*`), analyst-owned
  files (`src/agents/analyst/*`), or researcher-owned files (`src/agents/researcher/*`)
- Downstream consumers: `add-graph-orchestration` (wraps trader into a sequential `graph_flow::Task` node),
  `add-risk-management` (reads `trader_proposal` to evaluate risk)

## Cross-Owner Changes

- `src/agents/mod.rs` — owner: `add-project-foundation`.
  Justification: the foundation skeleton has `pub mod trader;` commented out (line 8). This change uncomments it
  so the trader module is compiled and accessible through the agent module path. This is a single-line edit that
  the foundation anticipated.
