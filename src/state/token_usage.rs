use serde::{Deserialize, Serialize};

/// Tracks token consumption per agent, per phase, and for the entire run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TokenUsageTracker {
    pub phase_usage: Vec<PhaseTokenUsage>,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
}

/// Token and timing data for a single workflow phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseTokenUsage {
    pub phase_name: String,
    pub agent_usage: Vec<AgentTokenUsage>,
    pub phase_prompt_tokens: u64,
    pub phase_completion_tokens: u64,
    pub phase_total_tokens: u64,
    pub phase_duration_ms: u64,
}

/// Token and timing data for a single agent invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTokenUsage {
    pub agent_name: String,
    pub model_id: String,
    /// `true` when the provider returned authoritative token counts.
    /// `false` means the agent ran (and failed or the provider did not report counts)
    /// — callers should not treat the numeric fields as reliable in that case.
    pub token_counts_available: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub latency_ms: u64,
}

impl AgentTokenUsage {
    /// Construct a best-effort usage record for an analyst that completed (possibly
    /// with an error) but did not produce authoritative token counts.
    ///
    /// Used by the fan-out orchestrator to ensure every analyst run — both successful
    /// and errored — contributes a usage entry to the phase record.
    pub fn unavailable(agent_name: &str, model_id: &str, latency_ms: u64) -> Self {
        Self {
            agent_name: agent_name.to_owned(),
            model_id: model_id.to_owned(),
            token_counts_available: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            latency_ms,
        }
    }
}
