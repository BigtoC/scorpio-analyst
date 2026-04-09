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
    let total_budget = policy.total_budget(timeout);
    prompt_with_retry_budget(agent, prompt, timeout, total_budget, policy).await
}

/// Send a one-shot prompt with timeout/retry and return extended details including usage.
pub async fn prompt_with_retry_details(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<PromptResponse>, TradingError> {
    let total_budget = policy.total_budget(timeout);
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
        let attempt_budget = prepare_attempt(
            agent,
            started_at,
            timeout,
            total_budget,
            policy,
            attempt,
            &RetryMessages {
                retrying: "retrying prompt after transient error",
                retry_budget: "prompt retry budget exhausted before next attempt",
                acquire_budget: "prompt retry budget exhausted before rate-limit acquire",
                exhausted: "prompt retry budget exhausted",
            },
        )
        .await?;
        rate_limit_wait_ms = rate_limit_wait_ms.saturating_add(attempt_budget.rate_limit_wait_ms);

        match tokio::time::timeout(attempt_budget.timeout, call_fn()).await {
            Ok(Ok(response)) => {
                return Ok(RetryOutcome {
                    result: response,
                    rate_limit_wait_ms,
                });
            }
            Ok(Err(err)) => {
                if attempt < policy.max_retries
                    && let Some(error) = transient_prompt_error_summary(&err)
                {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %error, "transient prompt error, will retry");
                    continue;
                }
                return Err(map_prompt_error_with_context(
                    agent.provider_name(),
                    agent.model_id(),
                    err,
                ));
            }
            Err(_elapsed) => {
                let err = attempt_timeout_error(started_at, agent, attempt, "prompt");
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
    let total_budget = policy.total_budget(timeout);
    chat_with_retry_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

pub(crate) async fn chat_with_retry_budget(
    agent: &LlmAgent,
    prompt: &str,
    chat_history: &[Message],
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
) -> Result<RetryOutcome<String>, TradingError> {
    let mut history = Vec::with_capacity(chat_history.len().saturating_add(2));
    history.extend_from_slice(chat_history);
    let outcome =
        chat_with_retry_details_budget(agent, prompt, &mut history, timeout, total_budget, policy)
            .await?;

    Ok(RetryOutcome {
        result: outcome.result.output,
        rate_limit_wait_ms: outcome.rate_limit_wait_ms,
    })
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
    let total_budget = policy.total_budget(timeout);
    chat_with_retry_details_budget(agent, prompt, chat_history, timeout, total_budget, policy).await
}

/// Budget-constrained variant of [`chat_with_retry_details`].
pub(crate) async fn chat_with_retry_details_budget(
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
            // Truncate any partial messages that were appended during the failed attempt.
            chat_history.truncate(initial_len);
        }
        let attempt_budget = prepare_attempt(
            agent,
            started_at,
            timeout,
            total_budget,
            policy,
            attempt,
            &RetryMessages {
                retrying: "retrying chat-details after transient error",
                retry_budget: "chat-details retry budget exhausted before next attempt",
                acquire_budget: "chat-details budget exhausted before rate-limit acquire",
                exhausted: "chat-details retry budget exhausted",
            },
        )
        .await?;
        rate_limit_wait_ms = rate_limit_wait_ms.saturating_add(attempt_budget.rate_limit_wait_ms);

        match tokio::time::timeout(
            attempt_budget.timeout,
            agent.chat_details(prompt, chat_history),
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
                // Restore caller-owned history on any failed attempt before retrying or returning.
                chat_history.truncate(initial_len);
                if attempt < policy.max_retries
                    && let Some(error) = transient_prompt_error_summary(&err)
                {
                    warn!(attempt, provider = agent.provider_name(), model = agent.model_id(), error = %error, "transient chat-details error, will retry");
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
                let err = attempt_timeout_error(started_at, agent, attempt, "chat-details");
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
    let total_budget = policy.total_budget(timeout);
    let started_at = Instant::now();
    let mut rate_limit_wait_ms: u64 = 0;

    for attempt in 0..=policy.max_retries {
        let attempt_budget = prepare_attempt(
            agent,
            started_at,
            timeout,
            total_budget,
            policy,
            attempt,
            &RetryMessages {
                retrying: "retrying typed prompt after transient error",
                retry_budget: "typed prompt retry budget exhausted before next attempt",
                acquire_budget: "typed prompt budget exhausted before rate-limit acquire",
                exhausted: "typed prompt retry budget exhausted",
            },
        )
        .await?;
        rate_limit_wait_ms = rate_limit_wait_ms.saturating_add(attempt_budget.rate_limit_wait_ms);

        match tokio::time::timeout(
            attempt_budget.timeout,
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
                let err = attempt_timeout_error(started_at, agent, attempt, "typed prompt");
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

pub(super) struct AttemptBudget {
    pub(super) timeout: Duration,
    pub(super) rate_limit_wait_ms: u64,
}

/// Log/error messages emitted by [`prepare_attempt`] for a given retry operation.
struct RetryMessages {
    retrying: &'static str,
    retry_budget: &'static str,
    acquire_budget: &'static str,
    exhausted: &'static str,
}

async fn prepare_attempt(
    agent: &LlmAgent,
    started_at: Instant,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
    attempt: u32,
    msgs: &RetryMessages,
) -> Result<AttemptBudget, TradingError> {
    if attempt > 0 {
        let delay = policy.delay_for_attempt(attempt - 1);
        if started_at.elapsed().saturating_add(delay) > total_budget {
            return Err(budget_timeout(started_at, msgs.retry_budget));
        }
        warn!(attempt, ?delay, "{}", msgs.retrying);
        tokio::time::sleep(delay).await;
    }

    let rate_limit_wait_ms =
        acquire_rate_limit_permit(agent, started_at, total_budget, msgs.acquire_budget).await?;
    let remaining_budget = total_budget.saturating_sub(started_at.elapsed());
    if remaining_budget.is_zero() {
        return Err(budget_timeout(started_at, msgs.exhausted));
    }

    Ok(AttemptBudget {
        timeout: timeout.min(remaining_budget),
        rate_limit_wait_ms,
    })
}

async fn acquire_rate_limit_permit(
    agent: &LlmAgent,
    started_at: Instant,
    total_budget: Duration,
    exhausted_message: &str,
) -> Result<u64, TradingError> {
    let Some(limiter) = agent.rate_limiter() else {
        return Ok(0);
    };

    let remaining = total_budget.saturating_sub(started_at.elapsed());
    if remaining.is_zero() {
        return Err(budget_timeout(started_at, exhausted_message));
    }

    let acquire_start = Instant::now();
    match tokio::time::timeout(remaining, limiter.acquire()).await {
        Ok(()) => Ok(acquire_start.elapsed().as_millis() as u64),
        Err(_) => Err(budget_timeout(
            started_at,
            "rate-limit acquire timed out (budget exhausted)",
        )),
    }
}

fn budget_timeout(started_at: Instant, message: &str) -> TradingError {
    TradingError::NetworkTimeout {
        elapsed: started_at.elapsed(),
        message: message.to_owned(),
    }
}

fn attempt_timeout_error(
    started_at: Instant,
    agent: &LlmAgent,
    attempt: u32,
    operation: &str,
) -> TradingError {
    TradingError::NetworkTimeout {
        elapsed: started_at.elapsed(),
        message: format!(
            "{operation} timed out on attempt {attempt} for model {}",
            agent.model_id()
        ),
    }
}

/// Classify whether a `PromptError` is likely transient (worth retrying).
///
/// Rate-limit and HTTP transport errors are considered transient.
/// Authentication, schema, and tool errors are permanent.
#[cfg(test)]
fn is_transient_error(err: &PromptError) -> bool {
    transient_prompt_error_summary(err).is_some()
}

/// Shared attempt-preparation logic exposed to sibling submodules (e.g. `text_retry`).
///
/// Uses fixed log messages appropriate for the "text prompt" operation.
pub(super) async fn prepare_attempt_text(
    agent: &LlmAgent,
    started_at: Instant,
    timeout: Duration,
    total_budget: Duration,
    policy: &RetryPolicy,
    attempt: u32,
) -> Result<AttemptBudget, TradingError> {
    prepare_attempt(
        agent,
        started_at,
        timeout,
        total_budget,
        policy,
        attempt,
        &RetryMessages {
            retrying: "retrying text prompt after transient error",
            retry_budget: "text prompt retry budget exhausted before next attempt",
            acquire_budget: "text prompt budget exhausted before rate-limit acquire",
            exhausted: "text prompt retry budget exhausted",
        },
    )
    .await
}

fn transient_prompt_error_summary(err: &PromptError) -> Option<String> {
    match err {
        PromptError::CompletionError(ce) => {
            let message = ce.to_string();
            is_transient_message(&message).then(|| sanitize_error_summary(&message))
        }
        // Tool errors and cancellations are not transient
        PromptError::ToolError(_)
        | PromptError::ToolServerError(_)
        | PromptError::MaxTurnsError { .. }
        | PromptError::PromptCancelled { .. } => None,
    }
}

pub(super) fn is_transient_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    // Rate-limit indicators from various providers
    message.contains("rate limit")
        || message.contains("429")
        || message.contains("too many requests")
        // Transient transport / server errors
        || message.contains("timeout")
        || message.contains("connection")
        || message.contains("500")
        || message.contains("502")
        || message.contains("503")
        || message.contains("504")
}

/// Shared retry predicate for `TradingError` variants, used by both typed and text retry paths.
///
/// Rate-limit and transient transport errors are retryable. Schema violations and
/// permanent provider errors are not.
pub(super) fn should_retry_trading_error(err: &TradingError) -> bool {
    match err {
        TradingError::NetworkTimeout { .. } | TradingError::RateLimitExceeded { .. } => true,
        // SchemaViolation is a permanent failure for a given LLM output — the same
        // prompt to the same model is unlikely to produce a valid response on retry,
        // and retrying wastes token budget. Fail fast on schema errors.
        TradingError::SchemaViolation { .. } => false,
        TradingError::Rig(message) => is_transient_message(message),
        TradingError::AnalystError { .. } | TradingError::Config(_) | TradingError::Storage(_) => {
            false
        }
        // GraphFlow errors originate from the orchestration layer, not from LLM providers,
        // so retrying the typed prompt won't help.
        TradingError::GraphFlow { .. } => false,
    }
}

fn should_retry_typed_error(err: &TradingError) -> bool {
    should_retry_trading_error(err)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RetryPolicy;
    use crate::state::{TradeAction, TradeProposal};
    use rig::OneOrMany;
    use rig::completion::Message;
    use rig::message::UserContent;

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
        let budget = policy.total_budget(Duration::from_secs(1));
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

    #[test]
    fn rig_timeout_message_is_retryable_for_typed_prompts() {
        let err =
            TradingError::Rig("provider=openai model=o3 summary=connection timeout".to_owned());
        assert!(should_retry_typed_error(&err));
    }

    #[test]
    fn rig_auth_message_is_not_retryable_for_typed_prompts() {
        let err = TradingError::Rig("provider=openai model=o3 summary=invalid api key".to_owned());
        assert!(!should_retry_typed_error(&err));
    }

    #[test]
    fn rate_limit_exceeded_is_retryable_for_typed_prompts() {
        let err = TradingError::RateLimitExceeded {
            provider: "openai".to_owned(),
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
                    rig::completion::CompletionError::ResponseError("rate limit 429".to_owned()),
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

    #[tokio::test]
    async fn chat_with_retry_details_truncates_partial_history_on_final_permanent_error() {
        let (agent, controller) = mock_llm_agent(
            "o3",
            vec![],
            vec![
                MockChatOutcome::PartialUserThenErr(PromptError::CompletionError(
                    rig::completion::CompletionError::ResponseError("rate limit 429".to_owned()),
                )),
                MockChatOutcome::PartialUserThenErr(PromptError::CompletionError(
                    rig::completion::CompletionError::ProviderError("invalid API key".to_owned()),
                )),
            ],
        );

        let mut history = vec![Message::User {
            content: OneOrMany::one(UserContent::text("initial context")),
        }];

        let err = chat_with_retry_details_budget(
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
        .unwrap_err();

        match err {
            TradingError::Rig(message) => assert!(message.contains("invalid API key")),
            other => panic!("expected TradingError::Rig, got {other:?}"),
        }

        assert_eq!(history.len(), 1);
        assert_eq!(controller.observed_history_lengths(), vec![1, 1]);
    }

    #[tokio::test]
    async fn chat_with_retry_retries_without_accumulating_history() {
        let (agent, controller) = mock_llm_agent(
            "o3",
            vec![],
            vec![
                MockChatOutcome::PartialUserThenErr(PromptError::CompletionError(
                    rig::completion::CompletionError::ResponseError("rate limit 429".to_owned()),
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

        let history = vec![Message::User {
            content: OneOrMany::one(UserContent::text("initial context")),
        }];

        let response = chat_with_retry_budget(
            &agent,
            "next prompt",
            &history,
            Duration::from_millis(50),
            Duration::from_millis(200),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.result, "Recovered response");
        assert_eq!(controller.observed_history_lengths(), vec![1, 1]);
        assert_eq!(controller.observed_history_ptrs().len(), 2);
        assert_eq!(
            controller.observed_history_ptrs()[0],
            controller.observed_history_ptrs()[1],
            "chat retries should reuse the same history buffer across attempts"
        );
    }

    #[tokio::test]
    async fn prompt_with_retry_retries_transient_error_once() {
        let (agent, _controller) = mock_llm_agent(
            "o3",
            vec![
                Err(PromptError::CompletionError(
                    rig::completion::CompletionError::ResponseError(
                        "HTTP 429 Too Many Requests".to_owned(),
                    ),
                )),
                Ok(mock_prompt_response(
                    "Recovered response",
                    rig::completion::Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        total_tokens: 15,
                        cached_input_tokens: 0,
                    },
                )),
            ],
            vec![],
        );

        let response = prompt_with_retry_budget(
            &agent,
            "next prompt",
            Duration::from_millis(50),
            Duration::from_millis(200),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.result, "Recovered response");
    }

    #[tokio::test]
    async fn prompt_with_retry_details_public_entrypoint_returns_usage_and_rate_limit_wait() {
        let (agent, _controller) = mock_llm_agent(
            "o3",
            vec![Ok(mock_prompt_response(
                "Detailed response",
                rig::completion::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                    total_tokens: 10,
                    cached_input_tokens: 0,
                },
            ))],
            vec![],
        );

        let response = prompt_with_retry_details(
            &agent,
            "next prompt",
            Duration::from_millis(50),
            &RetryPolicy {
                max_retries: 0,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.result.output, "Detailed response");
        assert_eq!(response.result.usage.total_tokens, 10);
        assert_eq!(response.rate_limit_wait_ms, 0);
    }

    #[tokio::test]
    async fn prompt_with_retry_public_entrypoint_maps_permanent_errors_without_retry() {
        let (agent, _controller) = mock_llm_agent(
            "o3",
            vec![Err(PromptError::CompletionError(
                rig::completion::CompletionError::ProviderError("invalid API key".to_owned()),
            ))],
            vec![],
        );

        let err = prompt_with_retry(
            &agent,
            "next prompt",
            Duration::from_millis(50),
            &RetryPolicy {
                max_retries: 3,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap_err();

        match err {
            TradingError::Rig(message) => {
                assert!(message.contains("provider=openai"));
                assert!(message.contains("model=o3"));
                assert!(message.contains("invalid API key"));
            }
            other => panic!("expected TradingError::Rig, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_with_retry_public_entrypoint_returns_attempt_timeout_after_budget_exhaustion() {
        let (agent, _controller) = mock_llm_agent("o3", vec![], vec![]);
        agent.set_prompt_delay(Duration::from_millis(25));

        let err = prompt_with_retry(
            &agent,
            "next prompt",
            Duration::from_millis(5),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
        )
        .await
        .unwrap_err();

        match err {
            TradingError::NetworkTimeout { message, .. } => {
                assert!(message.contains("prompt timed out on attempt 1"));
                assert!(message.contains("model o3"));
            }
            other => panic!("expected TradingError::NetworkTimeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_with_retry_public_entrypoint_surfaces_retry_budget_exhaustion_before_next_attempt()
     {
        let (agent, _controller) = mock_llm_agent(
            "o3",
            vec![Err(PromptError::CompletionError(
                rig::completion::CompletionError::ResponseError(
                    "HTTP 429 Too Many Requests".to_owned(),
                ),
            ))],
            vec![],
        );
        agent.set_prompt_delay(Duration::from_millis(18));

        let err = prompt_with_retry_budget(
            &agent,
            "next prompt",
            Duration::from_millis(20),
            Duration::from_millis(20),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(5),
            },
        )
        .await
        .unwrap_err();

        match err {
            TradingError::NetworkTimeout { message, .. } => {
                assert!(message.contains("prompt retry budget exhausted before next attempt"));
            }
            other => panic!("expected TradingError::NetworkTimeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_typed_with_retry_public_entrypoint_retries_transient_rig_errors() {
        let (agent, _controller) = mock_llm_agent("o3", vec![], vec![]);
        agent.push_typed_error(TradingError::Rig(
            "provider=openai model=o3 summary=connection timeout".to_owned(),
        ));
        agent.push_typed_ok(rig::agent::TypedPromptResponse::new(
            TradeProposal {
                action: TradeAction::Buy,
                target_price: 150.0,
                stop_loss: 140.0,
                confidence: 0.7,
                rationale: "Recovered after transient timeout".to_owned(),
                valuation_assessment: None,
                scenario_valuation: None,
            },
            rig::completion::Usage {
                input_tokens: 12,
                output_tokens: 8,
                total_tokens: 20,
                cached_input_tokens: 0,
            },
        ));

        let outcome = prompt_typed_with_retry::<TradeProposal>(
            &agent,
            "typed prompt",
            Duration::from_millis(50),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
            1,
        )
        .await
        .unwrap();

        assert_eq!(outcome.result.output.action, TradeAction::Buy);
        assert_eq!(outcome.result.usage.total_tokens, 20);
    }

    #[tokio::test]
    async fn prompt_typed_with_retry_public_entrypoint_does_not_retry_schema_violations() {
        let (agent, _controller) = mock_llm_agent("o3", vec![], vec![]);
        agent.push_typed_error(TradingError::SchemaViolation {
            message: "provider=openai model=o3: structured output could not be parsed".to_owned(),
        });
        agent.push_typed_ok(rig::agent::TypedPromptResponse::new(
            TradeProposal {
                action: TradeAction::Buy,
                target_price: 150.0,
                stop_loss: 140.0,
                confidence: 0.7,
                rationale: "Should not be reached".to_owned(),
                valuation_assessment: None,
                scenario_valuation: None,
            },
            rig::completion::Usage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                cached_input_tokens: 0,
            },
        ));

        let err = prompt_typed_with_retry::<TradeProposal>(
            &agent,
            "typed prompt",
            Duration::from_millis(50),
            &RetryPolicy {
                max_retries: 1,
                base_delay: Duration::from_millis(1),
            },
            1,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradingError::SchemaViolation { .. }));
        assert_eq!(agent.typed_attempts(), 1);
    }
}
