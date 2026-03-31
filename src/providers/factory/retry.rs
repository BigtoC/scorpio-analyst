//! Retry-wrapped LLM completion helpers with timeout and exponential backoff.
//!
//! - [`RetryOutcome`] — bundles a successful response with rate-limit wait metadata.
//! - [`prompt_with_retry`] / [`prompt_with_retry_details`] — one-shot prompt with retry.
//! - [`chat_with_retry`] / [`chat_with_retry_details`] — chat prompt with retry.
//! - [`prompt_typed_with_retry`] — typed structured-output prompt with retry.
//!
//! All functions apply `tokio::time::timeout` per attempt and exponential backoff
//! between attempts. Rate-limit permit acquisition is performed outside the per-attempt
//! timeout but bounded by the remaining total budget (Option C semantics).

use std::time::{Duration, Instant};

use rig::{
    agent::{PromptResponse, TypedPromptResponse},
    completion::{Message, PromptError},
};
use serde::de::DeserializeOwned;
use tracing::warn;

use crate::error::{RetryPolicy, TradingError};

use super::agent::LlmAgent;
use super::error::{map_prompt_error_with_context, sanitize_error_summary};

// ────────────────────────────────────────────────────────────────────────────
// RetryOutcome
// ────────────────────────────────────────────────────────────────────────────

/// The result of a retry-wrapped LLM call, bundling the response with the
/// total time spent waiting for rate-limit permits across all attempts.
#[derive(Debug)]
pub struct RetryOutcome<T> {
    /// The successful LLM response.
    pub result: T,
    /// Total milliseconds spent in `limiter.acquire()` across all attempts.
    pub rate_limit_wait_ms: u64,
}

// ────────────────────────────────────────────────────────────────────────────
// Prompt retry helpers
// ────────────────────────────────────────────────────────────────────────────

/// Send a one-shot prompt with timeout and exponential-backoff retry.
///
/// Each attempt is guarded by `tokio::time::timeout(timeout)`. Transient errors
/// (rate limit, timeout) trigger a retry up to `policy.max_retries` times. Permanent
/// errors fail immediately.
///
/// Rate-limit acquire is performed outside the per-attempt timeout but is bounded
/// by the remaining total budget (Option C semantics).
///
/// # Errors
///
/// - `TradingError::Rig` for permanent provider/transport failures.
/// - `TradingError::NetworkTimeout` if all attempts exceed the timeout.
pub async fn prompt_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<String>, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    prompt_with_retry_budget(agent, prompt, timeout, total_budget, policy).await
}

/// Send a one-shot prompt with timeout/retry and return extended details including usage.
pub async fn prompt_with_retry_details(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    prompt_with_retry_details_budget(agent, prompt, timeout, total_budget, policy).await
}

pub(crate) async fn prompt_with_retry_details_budget(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    retry_prompt_budget_loop(agent, timeout, total_budget, policy, || {
        agent.prompt_details(prompt)
    })
    .await
}

pub(crate) async fn prompt_with_retry_budget(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<String>, TradingError> {
    retry_prompt_budget_loop(agent, timeout, total_budget, policy, || {
        agent.prompt(prompt)
    })
    .await
}

/// Shared retry-loop core for [`prompt_with_retry_budget`] and
/// [`prompt_with_retry_details_budget`].
///
/// `call_fn` is invoked on each attempt and must return a `Future` that resolves to
/// `Result<R, PromptError>`. The two callers differ only in which `LlmAgent` method
/// they invoke (`prompt` vs `prompt_details`).
///
/// Before each attempt, acquires a rate-limit permit (if one is configured) outside
/// the per-attempt timeout, but bounded by the remaining total budget (Option C).
async fn retry_prompt_budget_loop<R, F, Fut>(
    agent: &LlmAgent,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
    call_fn: F,
) -> Result<RetryOutcome<R>, TradingError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<R, PromptError>>,
{
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "prompt retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(attempt, ?delay, "retrying prompt after transient error");
            tokio::time::sleep(delay).await;
        }

        // Acquire rate-limit permit outside the per-attempt timeout (Option C).
        // The acquire itself is bounded by remaining budget to avoid blocking forever.
        if let Some(limiter) = agent.rate_limiter() {
            let remaining = total_budget.saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "prompt retry budget exhausted before rate-limit acquire".to_owned(),
                });
            }
            let acquire_start = Instant::now();
            match tokio::time::timeout(remaining, limiter.acquire()).await {
                Ok(()) => {
                    rate_limit_wait_ms = rate_limit_wait_ms
                        .saturating_add(acquire_start.elapsed().as_millis() as u64);
                }
                Err(_) => {
                    return Err(TradingError::NetworkTimeout {
                        elapsed: started_at.elapsed(),
                        message: "rate-limit acquire timed out (budget exhausted)".to_owned(),
                    });
                }
            }
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "prompt retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, call_fn()).await {
            Ok(Ok(response)) => {
                return Ok(RetryOutcome {
                    result: response,
                    rate_limit_wait_ms,
                });
            }
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient prompt error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "prompt timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "prompt timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

// ────────────────────────────────────────────────────────────────────────────
// Chat retry helpers
// ────────────────────────────────────────────────────────────────────────────

/// Send a chat prompt (with history) with timeout and exponential-backoff retry.
///
/// Behaves identically to [`prompt_with_retry`] but passes `chat_history` to the agent.
/// The history is cloned on each attempt so retries replay the full context.
///
/// Rate-limit acquire is performed outside the per-attempt timeout but is bounded
/// by the remaining total budget (Option C semantics).
///
/// # Errors
///
/// Same as [`prompt_with_retry`].
pub async fn chat_with_retry(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &[Message],
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<String>, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    chat_with_retry_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

pub async fn chat_with_retry_budget(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &[Message],
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<String>, TradingError> {
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(attempt, ?delay, "retrying chat after transient error");
            tokio::time::sleep(delay).await;
        }

        // Acquire rate-limit permit outside the per-attempt timeout (Option C).
        if let Some(limiter) = agent.rate_limiter() {
            let remaining = total_budget.saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat retry budget exhausted before rate-limit acquire".to_owned(),
                });
            }
            let acquire_start = Instant::now();
            match tokio::time::timeout(remaining, limiter.acquire()).await {
                Ok(()) => {
                    rate_limit_wait_ms = rate_limit_wait_ms
                        .saturating_add(acquire_start.elapsed().as_millis() as u64);
                }
                Err(_) => {
                    return Err(TradingError::NetworkTimeout {
                        elapsed: started_at.elapsed(),
                        message: "rate-limit acquire timed out (budget exhausted)".to_owned(),
                    });
                }
            }
        }

        let history = chat_history.to_vec();
        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "chat retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, agent.chat(prompt, history)).await {
            Ok(Ok(response)) => {
                return Ok(RetryOutcome {
                    result: response,
                    rate_limit_wait_ms,
                });
            }
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient chat error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "chat timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "chat timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

/// Send a chat prompt (with mutable history) with timeout/retry and return response plus usage.
///
/// The `chat_history` is updated in place by appending each new message pair. This is the
/// correct API for multi-turn debates where callers maintain history across rounds.
///
/// Rate-limit acquire is performed outside the per-attempt timeout but is bounded
/// by the remaining total budget (Option C semantics).
///
/// # Errors
///
/// Same as [`chat_with_retry`].
pub async fn chat_with_retry_details(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &mut Vec<Message>,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let total_budget = total_request_budget(timeout, policy);
    chat_with_retry_details_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

/// Budget-constrained variant of [`chat_with_retry_details`].
pub async fn chat_with_retry_details_budget(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &mut Vec<Message>,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    // Snapshot the history length before each attempt so we can truncate on retry.
    let initial_len = chat_history.len();

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat-details retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(
                attempt,
                ?delay,
                "retrying chat-details after transient error"
            );
            // Truncate any partial messages that were appended during the failed attempt.
            chat_history.truncate(initial_len);
            tokio::time::sleep(delay).await;
        }

        // Acquire rate-limit permit outside the per-attempt timeout (Option C).
        if let Some(limiter) = agent.rate_limiter() {
            let remaining = total_budget.saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "chat-details budget exhausted before rate-limit acquire".to_owned(),
                });
            }
            let acquire_start = Instant::now();
            match tokio::time::timeout(remaining, limiter.acquire()).await {
                Ok(()) => {
                    rate_limit_wait_ms = rate_limit_wait_ms
                        .saturating_add(acquire_start.elapsed().as_millis() as u64);
                }
                Err(_) => {
                    return Err(TradingError::NetworkTimeout {
                        elapsed: started_at.elapsed(),
                        message: "rate-limit acquire timed out (budget exhausted)".to_owned(),
                    });
                }
            }
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "chat-details retry budget exhausted".to_owned(),
            });
        }
        let attempt_timeout = timeout.min(remaining_budget);

        match tokio::time::timeout(attempt_timeout, agent.chat_details(prompt, chat_history)).await
        {
            Ok(Ok(response)) => {
                return Ok(RetryOutcome {
                    result: response,
                    rate_limit_wait_ms,
                });
            }
            Ok(Err(err)) => {
                if is_transient_error(&err) && attempt < policy.max_retries {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %sanitize_error_summary(&err.to_string()), "transient chat-details error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                // On timeout, also truncate any partial messages.
                chat_history.truncate(initial_len);
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "chat-details timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    warn!(attempt, "chat-details timed out, will retry");
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

// ────────────────────────────────────────────────────────────────────────────
// Typed prompt retry
// ────────────────────────────────────────────────────────────────────────────

/// Prompt for a typed response and return usage metadata from the underlying agent loop.
pub async fn prompt_typed_with_retry<T>(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
    max_turns: usize,
) -> Result<RetryOutcome<TypedPromptResponse<T>>, TradingError>
where
    T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
{
    let total_budget = total_request_budget(timeout, policy);
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    for attempt in 0..=policy.max_retries {
        if attempt > 0 {
            let delay = policy.delay_for_attempt(attempt - 1);
            if started_at.elapsed().saturating_add(delay) > total_budget {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "typed prompt retry budget exhausted before next attempt".to_owned(),
                });
            }
            warn!(
                attempt,
                ?delay,
                "retrying typed prompt after transient error"
            );
            tokio::time::sleep(delay).await;
        }

        // Acquire rate-limit permit outside the per-attempt timeout (Option C).
        if let Some(limiter) = agent.rate_limiter() {
            let remaining = total_budget.saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                return Err(TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: "typed prompt budget exhausted before rate-limit acquire".to_owned(),
                });
            }
            let acquire_start = Instant::now();
            match tokio::time::timeout(remaining, limiter.acquire()).await {
                Ok(()) => {
                    rate_limit_wait_ms = rate_limit_wait_ms
                        .saturating_add(acquire_start.elapsed().as_millis() as u64);
                }
                Err(_) => {
                    return Err(TradingError::NetworkTimeout {
                        elapsed: started_at.elapsed(),
                        message: "rate-limit acquire timed out (budget exhausted)".to_owned(),
                    });
                }
            }
        }

        let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
        if remaining_budget.is_zero() {
            return Err(TradingError::NetworkTimeout {
                elapsed: started_at.elapsed(),
                message: "typed prompt retry budget exhausted".to_owned(),
            });
        }

        let attempt_timeout = timeout.min(remaining_budget);
        match tokio::time::timeout(
            attempt_timeout,
            agent.prompt_typed_details::<T>(prompt, max_turns),
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
                if should_retry_typed_error(&err) && attempt < policy.max_retries {
                    continue;
                }
                return Err(err);
            }
            Err(_elapsed) => {
                let err = TradingError::NetworkTimeout {
                    elapsed: started_at.elapsed(),
                    message: format!(
                        "typed prompt timed out on attempt {attempt} for model {}",
                        agent.model_id()
                    ),
                };
                if attempt < policy.max_retries {
                    continue;
                }
                return Err(err);
            }
        }
    }

    // The loop runs for `0..=max_retries` iterations. Every iteration either
    // returns early or continues. Reaching here requires zero iterations,
    // which is impossible because `max_retries >= 0` guarantees at least one.
    unreachable!("retry loop executed zero iterations — max_retries must be >= 0")
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Classify whether a `PromptError` is likely transient (worth retrying).
///
/// Rate-limit and HTTP transport errors are considered transient.
/// Authentication, schema, and tool errors are permanent.
fn is_transient_error(err: &PromptError) -> bool {
    match err {
        PromptError::CompletionError(ce) => {
            let msg = ce.to_string().to_lowercase();
            // Rate-limit indicators from various providers
            msg.contains("rate limit")
                || msg.contains("429")
                || msg.contains("too many requests")
                // Transient transport / server errors
                || msg.contains("timeout")
                || msg.contains("connection")
                || msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
        }
        // Tool errors and cancellations are not transient
        PromptError::ToolError(_)
        | PromptError::ToolServerError(_)
        | PromptError::MaxTurnsError { .. }
        | PromptError::PromptCancelled { .. } => false,
    }
}

fn should_retry_typed_error(err: &TradingError) -> bool {
    match err {
        TradingError::NetworkTimeout { .. } | TradingError::RateLimitExceeded { .. } => true,
        // SchemaViolation is a permanent failure for a given LLM output — the same
        // prompt to the same model is unlikely to produce a valid response on retry,
        // and retrying wastes token budget. Fail fast on schema errors.
        TradingError::SchemaViolation { .. } => false,
        TradingError::Rig(message) => {
            let msg = message.to_ascii_lowercase();
            msg.contains("rate limit")
                || msg.contains("429")
                || msg.contains("too many requests")
                || msg.contains("timeout")
                || msg.contains("connection")
                || msg.contains("500")
                || msg.contains("502")
                || msg.contains("503")
                || msg.contains("504")
        }
        TradingError::AnalystError { .. } | TradingError::Config(_) | TradingError::Storage(_) => {
            false
        }
        // GraphFlow errors originate from the orchestration layer, not from LLM providers,
        // so retrying the typed prompt won't help.
        TradingError::GraphFlow { .. } => false,
    }
}

fn total_request_budget(timeout: Duration, policy: &RetryPolicy) -> Duration {
    let attempts = policy.max_retries.saturating_add(1);
    let base_budget = timeout.saturating_mul(attempts);
    let backoff_budget = (0..policy.max_retries).fold(Duration::ZERO, |acc, attempt| {
        acc.saturating_add(policy.delay_for_attempt(attempt))
    });
    base_budget.saturating_add(backoff_budget)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RetryPolicy;
    use rig::completion::Message;
    use rig::message::UserContent;
    use rig::OneOrMany;

    use super::super::agent::{MockChatOutcome, mock_llm_agent, mock_prompt_response};

    // ── Transient error classification ───────────────────────────────────

    #[test]
    fn rate_limit_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "rate limit exceeded".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn http_429_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "HTTP 429 Too Many Requests".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn server_500_error_is_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ResponseError(
            "Internal server error 500".to_owned(),
        ));
        assert!(is_transient_error(&err));
    }

    #[test]
    fn auth_error_is_not_transient() {
        let err = PromptError::CompletionError(rig::completion::CompletionError::ProviderError(
            "invalid API key".to_owned(),
        ));
        assert!(!is_transient_error(&err));
    }

    #[test]
    fn tool_error_is_not_transient() {
        use rig::tool::ToolSetError;
        let err = PromptError::ToolError(ToolSetError::ToolNotFoundError("foo".to_owned()));
        assert!(!is_transient_error(&err));
    }

    // ── Retry policy arithmetic ──────────────────────────────────────────

    #[test]
    fn retry_policy_delay_arithmetic() {
        let policy = RetryPolicy {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
        };
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(400));
    }

    #[test]
    fn total_request_budget_includes_retry_delays() {
        let policy = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(100),
        };
        let budget = total_request_budget(Duration::from_secs(1), &policy);
        assert_eq!(budget, Duration::from_millis(3300));
    }

    // ── Schema violation retry policy ────────────────────────────────────

    #[test]
    fn schema_violation_is_not_retryable() {
        let err = TradingError::SchemaViolation {
            message: "bad output".to_owned(),
        };
        assert!(
            !should_retry_typed_error(&err),
            "SchemaViolation must not be retried"
        );
    }

    #[test]
    fn network_timeout_is_retryable() {
        let err = TradingError::NetworkTimeout {
            elapsed: Duration::from_secs(30),
            message: "timed out".to_owned(),
        };
        assert!(should_retry_typed_error(&err));
    }

    // ── Integration: chat_with_retry_details ─────────────────────────────

    #[tokio::test]
    async fn chat_with_retry_details_retries_and_truncates_partial_history() {
        let (agent, controller) = mock_llm_agent(
            "o3",
            vec![],
            vec![
                MockChatOutcome::PartialUserThenErr(PromptError::CompletionError(
                    rig::completion::CompletionError::ResponseError(
                        "rate limit 429".to_owned(),
                    ),
                )),
                MockChatOutcome::Ok(mock_prompt_response(
                    "Recovered response",
                    rig::completion::Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                        cached_input_tokens: 0,
                    },
                )),
            ],
        );

        let mut history = vec![Message::User {
            content: OneOrMany::one(UserContent::text("initial context")),
        }];

        let response = chat_with_retry_details_budget(
            &agent,
            "next prompt",
            &mut history,
            Duration::from_millis(50),
            Duration::from_millis(200),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.result.output, "Recovered response");
        assert_eq!(response.result.usage.total_tokens, 15);
        assert_eq!(response.result.usage.output_tokens, 5);
        assert_eq!(history.len(), 3);
        assert_eq!(controller.observed_history_lengths(), vec![1, 1]);
    }
}
