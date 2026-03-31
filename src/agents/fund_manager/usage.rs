use std::time::Instant;

use crate::state::AgentTokenUsage;

pub(super) fn usage_from_response(
    agent_name: &str,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
    rate_limit_wait_ms: u64,
) -> AgentTokenUsage {
    AgentTokenUsage {
        agent_name: agent_name.to_owned(),
        model_id: model_id.to_owned(),
        token_counts_available: usage.total_tokens > 0
            || usage.input_tokens > 0
            || usage.output_tokens > 0,
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        latency_ms: started_at.elapsed().as_millis() as u64,
        rate_limit_wait_ms,
    }
}
