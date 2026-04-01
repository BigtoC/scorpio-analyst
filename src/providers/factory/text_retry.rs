//! Retry-wrapped text (non-structured) LLM completion helper.
//!
//! - [`prompt_text_with_retry`] — tool-enabled one-shot text prompt with timeout and
//!   exponential-backoff retry, returning a [`PromptResponse`] so callers have both
//!   `output` text and `usage` details.
//!
//! This is the fallback path used by analysts when a provider does not support
//! structured outputs.  The implementation mirrors [`super::retry::prompt_typed_with_retry`]
//! but calls [`LlmAgent::prompt_text_details`] instead of the typed agent path.

use std::time::{Duration, Instant};

use rig::agent::PromptResponse;
use tracing::warn;

use crate::error::{RetryPolicy, TradingError};

use super::agent::LlmAgent;
use super::retry::{RetryOutcome, prepare_attempt_text};

// ────────────────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────────────────

/// Send a tool-enabled text prompt with timeout and exponential-backoff retry.
///
/// Returns a [`RetryOutcome<PromptResponse>`] so callers have both the raw text
/// output and the provider-reported usage statistics.
///
/// The `max_turns` parameter is forwarded to the underlying agent's tool-turn loop
/// so multi-step tool calls are honoured on each attempt.
///
/// # Errors
///
/// - [`TradingError::NetworkTimeout`] if all attempts exceed the per-attempt timeout.
/// - [`TradingError::Rig`] for permanent provider/transport failures.
pub async fn prompt_text_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    max_turns: usize,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let total_budget = policy.total_budget(timeout);
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    for attempt in 0..=policy.max_retries {
        let attempt_budget = prepare_attempt_text(
            agent,
            started_at,
            timeout,
            total_budget,
            policy,
            attempt,
        )
        .await?;
        rate_limit_wait_ms = rate_limit_wait_ms.saturating_add(attempt_budget.rate_limit_wait_ms);

        match tokio::time::timeout(
            attempt_budget.timeout,
            agent.prompt_text_details(prompt, max_turns),
        )
        .await
        {
            Ok(Ok(response)) => {
                return Ok(RetryOutcome {
                    result: response,
                    rate_limit_wait_ms,
                });
            }
            Ok(Err(err)) => {
                if should_retry_text_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %err, "transient text prompt error, will retry");
                    continue;
                }
                return Err(err);
            }
            Err(_elapsed) => {
                let err = text_timeout_error(started_at, agent, attempt);
                if attempt < policy.max_retries {
                    warn!(attempt, "text prompt timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

fn should_retry_text_error(err: &TradingError) -> bool {
    match err {
        TradingError::NetworkTimeout { .. } | TradingError::RateLimitExceeded { .. } => true,
        TradingError::Rig(message) => super::retry::is_transient_message_pub(message),
        TradingError::AnalystError { .. }
        | TradingError::Config(_)
        | TradingError::Storage(_)
        | TradingError::SchemaViolation { .. }
        | TradingError::GraphFlow { .. } => false,
    }
}

fn text_timeout_error(started_at: Instant, agent: &LlmAgent, attempt: u32) -> TradingError {
    TradingError::NetworkTimeout {
        elapsed: started_at.elapsed(),
        message: format!(
            "text prompt timed out on attempt {attempt} for model {}",
            agent.model_id()
        ),
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rig::completion::Usage;

    use crate::error::RetryPolicy;

    use super::super::agent_test_support::{
        mock_llm_agent_with_provider, prompt_attempts, text_turn_attempts,
    };
    use super::*;

    fn zero_usage() -> Usage {
        Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
        }
    }

    fn fast_policy(max_retries: u32) -> RetryPolicy {
        RetryPolicy {
            max_retries,
            base_delay: Duration::from_millis(1),
        }
    }

    // ── Test 1: usage is returned from prompt_text_details path ──────────

    #[tokio::test]
    async fn prompt_text_with_retry_returns_usage_from_prompt_details() {
        let usage = Usage {
            input_tokens: 5,
            output_tokens: 3,
            total_tokens: 8,
            cached_input_tokens: 0,
        };
        let (agent, _ctrl) = mock_llm_agent_with_provider("test-model", vec![], vec![]);
        // Response must be on the text_turn queue (not the one-shot prompt queue)
        agent.push_text_turn_ok(PromptResponse::new("hello", usage.clone()));

        let outcome = prompt_text_with_retry(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(0),
            1,
        )
        .await
        .unwrap();

        assert_eq!(outcome.result.output, "hello");
        assert_eq!(outcome.result.usage.total_tokens, 8);
        assert_eq!(outcome.result.usage.input_tokens, 5);
        assert_eq!(outcome.result.usage.output_tokens, 3);
        assert_eq!(outcome.rate_limit_wait_ms, 0);
    }

    // ── Test 2: transient errors are retried ─────────────────────────────

    #[tokio::test]
    async fn prompt_text_with_retry_retries_transient_prompt_errors() {
        let (agent, _ctrl) = mock_llm_agent_with_provider("test-model", vec![], vec![]);
        // First attempt: transient Rig error
        agent.push_text_turn_error(TradingError::Rig(
            "connection timeout on attempt 0".to_owned(),
        ));
        // Second attempt: success
        agent.push_text_turn_ok(PromptResponse::new(
            "recovered",
            Usage {
                input_tokens: 2,
                output_tokens: 1,
                total_tokens: 3,
                cached_input_tokens: 0,
            },
        ));

        let outcome = prompt_text_with_retry(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(1),
            1,
        )
        .await
        .unwrap();

        assert_eq!(outcome.result.output, "recovered");
        assert_eq!(text_turn_attempts(&agent), 2);
    }

    // ── Test 3: timeout error includes "text prompt" in message ──────────

    #[tokio::test]
    async fn prompt_text_with_retry_times_out_with_text_prompt_operation_name() {
        let (agent, _ctrl) = mock_llm_agent_with_provider("slow-model", vec![], vec![]);
        // Delay every text_turn response by 100ms so it times out
        agent.set_text_turn_delay(Duration::from_millis(100));

        let err = prompt_text_with_retry(
            &agent,
            "prompt",
            Duration::from_millis(5),
            &fast_policy(0),
            1,
        )
        .await
        .unwrap_err();

        match err {
            TradingError::NetworkTimeout { message, .. } => {
                assert!(
                    message.contains("text prompt"),
                    "expected 'text prompt' in timeout message, got: {message}"
                );
                assert!(
                    message.contains("slow-model"),
                    "expected model id in timeout message, got: {message}"
                );
            }
            other => panic!("expected NetworkTimeout, got {other:?}"),
        }
    }

    // ── Test 4: max_turns is preserved for tool-enabled requests ─────────

    #[tokio::test]
    async fn prompt_text_with_retry_preserves_max_turns_for_tool_enabled_requests() {
        let (agent, _ctrl) = mock_llm_agent_with_provider("test-model", vec![], vec![]);
        agent.push_text_turn_ok(PromptResponse::new("result", zero_usage()));

        // The important thing: max_turns=5 must reach the underlying agent
        prompt_text_with_retry(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(0),
            5,
        )
        .await
        .unwrap();

        // The agent records what max_turns it received; verify it was 5
        let observed = agent.observed_max_turns();
        assert_eq!(
            observed,
            vec![5],
            "expected max_turns=5 to be forwarded to the agent"
        );
    }

    // ── Test 5: text_turn path, not one-shot prompt_details ──────────────

    #[tokio::test]
    async fn prompt_text_with_retry_uses_text_turn_agent_path_not_one_shot_prompt_details() {
        let (agent, _ctrl) = mock_llm_agent_with_provider("test-model", vec![], vec![]);
        // Push a response on the text_turn queue (NOT the one-shot prompt queue)
        agent.push_text_turn_ok(PromptResponse::new("from text turn", zero_usage()));

        let outcome = prompt_text_with_retry(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(0),
            1,
        )
        .await
        .unwrap();

        // text_turn path was used
        assert_eq!(text_turn_attempts(&agent), 1);
        // one-shot prompt path was NOT used
        assert_eq!(prompt_attempts(&agent), 0);
        assert_eq!(outcome.result.output, "from text turn");
    }
}
