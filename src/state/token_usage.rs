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
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub latency_ms: u64,
}
