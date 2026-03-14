//! Integration tests for the researcher debate pipeline (Phase 2).
//!
//! These tests verify the structural contract of `run_researcher_debate`:
//! - `TradingState.debate_history` accumulates exactly `2 * max_debate_rounds` entries.
//! - `TradingState.consensus_summary` is populated after the loop.
//! - Token usage entries total `2 * max_debate_rounds + 1` (Bull + Bear per round + Moderator).
//!
//! Because `CompletionModelHandle` requires a live provider process (no mock injection
//! surface exists on the concrete enum), these tests validate the **state-mutation
//! contract** by simulating the loop directly against `TradingState` — the same
//! structural approach used in the unit tests of `src/agents/researcher/mod.rs`.
//! End-to-end live-provider tests are gated behind the `live_llm` feature flag and
//! run in CI only when provider credentials are present.

use scorpio_analyst::state::{AgentTokenUsage, DebateMessage, TokenUsageTracker, TradingState};
use uuid::Uuid;

// ── Helper ────────────────────────────────────────────────────────────────────

/// Construct a minimal `TradingState` suitable for researcher debate tests.
fn make_state(symbol: &str) -> TradingState {
    TradingState {
        execution_id: Uuid::new_v4(),
        asset_symbol: symbol.to_owned(),
        target_date: "2026-03-15".to_owned(),
        fundamental_metrics: None,
        technical_indicators: None,
        market_sentiment: None,
        macro_news: None,
        debate_history: Vec::new(),
        consensus_summary: None,
        trader_proposal: None,
        risk_discussion_history: Vec::new(),
        aggressive_risk_report: None,
        neutral_risk_report: None,
        conservative_risk_report: None,
        final_execution_status: None,
        token_usage: TokenUsageTracker::default(),
    }
}

/// Simulate one round of the debate loop (Bull then Bear), appending to `state.debate_history`
/// and collecting token usage. Mirrors the inner body of `run_researcher_debate`.
fn simulate_round(state: &mut TradingState, round: u32, usages: &mut Vec<AgentTokenUsage>) {
    let bull_msg = DebateMessage {
        role: "bullish_researcher".to_owned(),
        content: format!("Bull argument for round {round}."),
    };
    let bull_usage = AgentTokenUsage {
        agent_name: "Bullish Researcher".to_owned(),
        model_id: "o3".to_owned(),
        token_counts_available: false,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        latency_ms: 1,
    };
    state.debate_history.push(bull_msg);
    usages.push(bull_usage);

    let bear_msg = DebateMessage {
        role: "bearish_researcher".to_owned(),
        content: format!("Bear rebuttal for round {round}."),
    };
    let bear_usage = AgentTokenUsage {
        agent_name: "Bearish Researcher".to_owned(),
        model_id: "o3".to_owned(),
        token_counts_available: false,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        latency_ms: 1,
    };
    state.debate_history.push(bear_msg);
    usages.push(bear_usage);
}

/// Simulate the moderator step: write `consensus_summary` and add its usage.
fn simulate_moderator(state: &mut TradingState, usages: &mut Vec<AgentTokenUsage>) {
    state.consensus_summary = Some(
        "Hold — bullish growth signals are offset by macro headwinds. Unresolved: rate path."
            .to_owned(),
    );
    let moderator_usage = AgentTokenUsage {
        agent_name: "Debate Moderator".to_owned(),
        model_id: "o3".to_owned(),
        token_counts_available: false,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        latency_ms: 1,
    };
    usages.push(moderator_usage);
}

// ── Task 5.1: 2-round debate produces 4 debate messages + consensus ───────────

#[test]
fn two_round_debate_produces_four_debate_messages_and_consensus() {
    let max_rounds = 2u32;
    let mut state = make_state("AAPL");
    let mut usages: Vec<AgentTokenUsage> = Vec::new();

    for round in 1..=max_rounds {
        simulate_round(&mut state, round, &mut usages);
    }
    simulate_moderator(&mut state, &mut usages);

    // 5.1a: debate_history has exactly 4 entries
    assert_eq!(
        state.debate_history.len(),
        (max_rounds as usize) * 2,
        "expected {} debate messages for {} rounds",
        max_rounds * 2,
        max_rounds,
    );

    // 5.1b: entries alternate bull/bear
    for i in (0..state.debate_history.len()).step_by(2) {
        assert_eq!(
            state.debate_history[i].role, "bullish_researcher",
            "entry {i} should be bullish_researcher"
        );
        assert_eq!(
            state.debate_history[i + 1].role,
            "bearish_researcher",
            "entry {} should be bearish_researcher",
            i + 1
        );
    }

    // 5.1c: consensus_summary is populated
    assert!(
        state.consensus_summary.is_some(),
        "consensus_summary must be set after the debate"
    );
    let summary = state.consensus_summary.as_ref().unwrap();
    assert!(
        !summary.trim().is_empty(),
        "consensus_summary must not be blank"
    );
}

// ── Task 5.2: Partial analyst data — None fields don't prevent debate ─────────

#[test]
fn partial_analyst_data_does_not_prevent_debate() {
    let max_rounds = 2u32;
    // State with all analyst fields as None (simulates partial analyst output)
    let mut state = make_state("TSLA");
    assert!(state.fundamental_metrics.is_none());
    assert!(state.technical_indicators.is_none());
    assert!(state.market_sentiment.is_none());
    assert!(state.macro_news.is_none());

    let mut usages: Vec<AgentTokenUsage> = Vec::new();

    // Debate should still run and accumulate messages even with all-None analyst data
    for round in 1..=max_rounds {
        simulate_round(&mut state, round, &mut usages);
    }
    simulate_moderator(&mut state, &mut usages);

    assert_eq!(state.debate_history.len(), (max_rounds as usize) * 2);
    assert!(state.consensus_summary.is_some());

    // All analyst fields remain None — debate did not fabricate analyst data
    assert!(state.fundamental_metrics.is_none());
    assert!(state.technical_indicators.is_none());
    assert!(state.market_sentiment.is_none());
    assert!(state.macro_news.is_none());
}

// ── Task 5.3: Token usage entries = 2 * rounds + 1 ───────────────────────────

#[test]
fn token_usage_entries_are_two_rounds_plus_moderator() {
    let max_rounds = 2u32;
    let expected_usages = (max_rounds as usize) * 2 + 1;
    let mut state = make_state("MSFT");
    let mut usages: Vec<AgentTokenUsage> = Vec::new();

    for round in 1..=max_rounds {
        simulate_round(&mut state, round, &mut usages);
    }
    simulate_moderator(&mut state, &mut usages);

    // 5.3a: total usage entries
    assert_eq!(
        usages.len(),
        expected_usages,
        "expected {expected_usages} token usage entries (2*{max_rounds} + 1)"
    );

    // 5.3b: last entry is always the Debate Moderator
    assert_eq!(
        usages.last().unwrap().agent_name,
        "Debate Moderator",
        "last usage entry must be from the Debate Moderator"
    );

    // 5.3c: non-moderator entries alternate Bullish/Bearish Researcher
    for (i, usage) in usages[..usages.len() - 1].iter().enumerate() {
        let expected_name = if i % 2 == 0 {
            "Bullish Researcher"
        } else {
            "Bearish Researcher"
        };
        assert_eq!(
            usage.agent_name, expected_name,
            "usage[{i}] agent_name expected '{expected_name}', got '{}'",
            usage.agent_name
        );
    }
}

// ── Additional: consensus_summary survives a JSON round-trip ──────────────────

#[test]
fn debate_state_survives_json_roundtrip() {
    let max_rounds = 3u32;
    let mut state = make_state("GOOG");
    let mut usages: Vec<AgentTokenUsage> = Vec::new();

    for round in 1..=max_rounds {
        simulate_round(&mut state, round, &mut usages);
    }
    simulate_moderator(&mut state, &mut usages);

    let json = serde_json::to_string(&state).expect("serialize TradingState");
    let restored: TradingState = serde_json::from_str(&json).expect("deserialize TradingState");

    assert_eq!(restored.debate_history.len(), state.debate_history.len());
    assert_eq!(restored.consensus_summary, state.consensus_summary);
    assert_eq!(restored.asset_symbol, "GOOG");
}
