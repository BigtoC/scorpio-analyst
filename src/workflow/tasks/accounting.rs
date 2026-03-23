use graph_flow::Context;
use tracing::info;

use crate::{
    state::{AgentTokenUsage, PhaseTokenUsage, TradingState},
    workflow::tasks::common::{
        DEBATE_USAGE_PREFIX, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS,
        KEY_RISK_ROUND, RISK_USAGE_PREFIX, read_round_usage,
    },
};

/// Shared accounting for the researcher debate moderator.
pub(super) async fn debate_moderator_accounting(
    context: &Context,
    state: &mut TradingState,
    mod_usage: &AgentTokenUsage,
    phase_start: &std::time::Instant,
) -> bool {
    let max_rounds: u32 = context.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);

    let new_round = if max_rounds > 0 {
        let current_round: u32 = context.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        let new_round = current_round + 1;
        context.set(KEY_DEBATE_ROUND, new_round).await;
        info!(
            round = new_round,
            max_rounds,
            phase = 2,
            "debate round complete"
        );

        let bull_usage = read_round_usage(
            context,
            DEBATE_USAGE_PREFIX,
            new_round,
            "bull",
            "Bullish Researcher",
        )
        .await;
        let bear_usage = read_round_usage(
            context,
            DEBATE_USAGE_PREFIX,
            new_round,
            "bear",
            "Bearish Researcher",
        )
        .await;

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: format!("Researcher Debate Round {new_round}"),
            agent_usage: vec![bull_usage.clone(), bear_usage.clone()],
            phase_prompt_tokens: bull_usage.prompt_tokens + bear_usage.prompt_tokens,
            phase_completion_tokens: bull_usage.completion_tokens + bear_usage.completion_tokens,
            phase_total_tokens: bull_usage.total_tokens + bear_usage.total_tokens,
            phase_duration_ms: bull_usage.latency_ms + bear_usage.latency_ms,
        });

        new_round
    } else {
        0
    };

    let is_final = new_round >= max_rounds;
    if is_final {
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Researcher Debate Moderation".to_owned(),
            agent_usage: vec![mod_usage.clone()],
            phase_prompt_tokens: mod_usage.prompt_tokens,
            phase_completion_tokens: mod_usage.completion_tokens,
            phase_total_tokens: mod_usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });
    }

    is_final
}

/// Shared accounting for the risk discussion moderator.
pub(super) async fn risk_moderator_accounting(
    context: &Context,
    state: &mut TradingState,
    mod_usage: &AgentTokenUsage,
    phase_start: &std::time::Instant,
) -> bool {
    let max_rounds: u32 = context.get(KEY_MAX_RISK_ROUNDS).await.unwrap_or(0);

    let new_round = if max_rounds > 0 {
        let current_round: u32 = context.get(KEY_RISK_ROUND).await.unwrap_or(0);
        let new_round = current_round + 1;
        context.set(KEY_RISK_ROUND, new_round).await;
        info!(
            round = new_round,
            max_rounds,
            phase = 4,
            "risk round complete"
        );

        let agg_usage = read_round_usage(
            context,
            RISK_USAGE_PREFIX,
            new_round,
            "agg",
            "Aggressive Risk",
        )
        .await;
        let con_usage = read_round_usage(
            context,
            RISK_USAGE_PREFIX,
            new_round,
            "con",
            "Conservative Risk",
        )
        .await;
        let neu_usage =
            read_round_usage(context, RISK_USAGE_PREFIX, new_round, "neu", "Neutral Risk").await;

        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: format!("Risk Discussion Round {new_round}"),
            agent_usage: vec![agg_usage.clone(), con_usage.clone(), neu_usage.clone()],
            phase_prompt_tokens: agg_usage.prompt_tokens
                + con_usage.prompt_tokens
                + neu_usage.prompt_tokens,
            phase_completion_tokens: agg_usage.completion_tokens
                + con_usage.completion_tokens
                + neu_usage.completion_tokens,
            phase_total_tokens: agg_usage.total_tokens
                + con_usage.total_tokens
                + neu_usage.total_tokens,
            phase_duration_ms: agg_usage.latency_ms + con_usage.latency_ms + neu_usage.latency_ms,
        });

        new_round
    } else {
        0
    };

    let is_final = new_round >= max_rounds;
    if is_final {
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Risk Discussion Moderation".to_owned(),
            agent_usage: vec![mod_usage.clone()],
            phase_prompt_tokens: mod_usage.prompt_tokens,
            phase_completion_tokens: mod_usage.completion_tokens,
            phase_total_tokens: mod_usage.total_tokens,
            phase_duration_ms: phase_start.elapsed().as_millis() as u64,
        });
    }

    is_final
}
