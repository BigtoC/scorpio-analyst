use std::time::Instant;

use crate::state::AgentTokenUsage;

/// Build an [`AgentTokenUsage`] from a provider completion usage payload.
pub(crate) fn agent_token_usage_from_completion(
    agent_name: &str,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
    rate_limit_wait_ms: u64,
) -> AgentTokenUsage {
    let prompt_tokens =
        usage.input_tokens + usage.cached_input_tokens + usage.cache_creation_input_tokens;

    AgentTokenUsage {
        agent_name: agent_name.to_owned(),
        model_id: model_id.to_owned(),
        token_counts_available: usage.total_tokens > 0
            || usage.input_tokens > 0
            || usage.output_tokens > 0
            || usage.cached_input_tokens > 0
            || usage.cache_creation_input_tokens > 0,
        prompt_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        latency_ms: started_at.elapsed().as_millis() as u64,
        rate_limit_wait_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::agent_token_usage_from_completion;
    use std::time::Instant;

    #[test]
    fn usage_from_completion_folds_cache_tokens_into_prompt_tokens() {
        let usage = rig::completion::Usage {
            input_tokens: 80,
            output_tokens: 20,
            total_tokens: 140,
            cached_input_tokens: 30,
            cache_creation_input_tokens: 10,
        };

        let result =
            agent_token_usage_from_completion("Agent", "model-x", usage, Instant::now(), 0);

        assert_eq!(result.prompt_tokens, 120);
        assert_eq!(result.completion_tokens, 20);
        assert_eq!(result.total_tokens, 140);
        assert!(result.token_counts_available);
    }

    #[test]
    fn usage_from_completion_marks_cache_only_counts_available() {
        let usage = rig::completion::Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 12,
        };

        let result =
            agent_token_usage_from_completion("Agent", "model-x", usage, Instant::now(), 0);

        assert_eq!(result.prompt_tokens, 12);
        assert!(result.token_counts_available);
    }
}
