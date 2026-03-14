use std::time::Duration;

use thiserror::Error;

use crate::config::LlmConfig;

/// Unified error type for the trading system.
#[derive(Debug, Error)]
pub enum TradingError {
    /// An analyst task failed to produce its output.
    #[error("analyst error ({agent}): {message}")]
    AnalystError { agent: String, message: String },

    /// An external API enforced a rate limit.
    #[error("rate limit exceeded for {provider}")]
    RateLimitExceeded { provider: String },

    /// A network request timed out.
    #[error("network timeout after {elapsed:?}: {message}")]
    NetworkTimeout { elapsed: Duration, message: String },

    /// The LLM returned data that could not be parsed into the expected schema.
    #[error("schema violation: {message}")]
    SchemaViolation { message: String },

    /// An error originating from the `rig` LLM framework.
    #[error("rig error: {0}")]
    Rig(String),

    /// Configuration is invalid or missing required values.
    #[error("config error: {0}")]
    Config(#[from] anyhow::Error),
}

/// Retry parameters for exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}

impl RetryPolicy {
    /// Build a `RetryPolicy` from the LLM configuration values.
    pub fn from_config(cfg: &LlmConfig) -> Self {
        Self {
            max_retries: cfg.retry_max_retries,
            base_delay: Duration::from_millis(cfg.retry_base_delay_ms),
        }
    }

    /// Calculate the delay for a given attempt (0-indexed), using exponential backoff.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        self.base_delay * 2u32.saturating_pow(attempt)
    }

    /// Total wall-clock budget required to execute all attempts including backoff delays.
    ///
    /// Use this as the outer `tokio::time::timeout` duration so the per-attempt
    /// timeout and retry backoff fit entirely within the outer limit.
    pub fn total_budget(&self, base_timeout: Duration) -> Duration {
        let attempts = self.max_retries.saturating_add(1);
        let base_budget = base_timeout.saturating_mul(attempts);
        let backoff_budget = (0..self.max_retries).fold(Duration::ZERO, |acc, attempt| {
            acc.saturating_add(self.delay_for_attempt(attempt))
        });
        base_budget.saturating_add(backoff_budget)
    }
}

/// Result of an analyst fan-out where some agents may have failed.
///
/// Degradation rules:
/// - 1 failure: continue with partial data
/// - 2+ failures: abort the cycle
///
/// Pass the names of the failed agents; the error message includes their names
/// and the count so upstream callers can diagnose which analysts failed.
pub fn check_analyst_degradation(
    total: usize,
    failed_agents: &[String],
) -> Result<(), TradingError> {
    let failures = failed_agents.len();
    if failures >= 2 || (total > 0 && failures == total) {
        return Err(TradingError::AnalystError {
            agent: failed_agents.join(", "),
            message: format!("{failures}/{total} analysts failed — aborting cycle"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_exponential() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for_attempt(0), Duration::from_millis(500));
        assert_eq!(policy.delay_for_attempt(1), Duration::from_millis(1000));
        assert_eq!(policy.delay_for_attempt(2), Duration::from_millis(2000));
    }

    #[test]
    fn degradation_allows_single_failure() {
        assert!(check_analyst_degradation(4, &["Fundamental Analyst".to_owned()]).is_ok());
    }

    #[test]
    fn degradation_aborts_on_two_failures() {
        let failed = vec!["Fundamental Analyst".to_owned(), "News Analyst".to_owned()];
        assert!(check_analyst_degradation(4, &failed).is_err());
    }

    #[test]
    fn degradation_aborts_on_total_failure() {
        let failed = vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
        ];
        assert!(check_analyst_degradation(4, &failed).is_err());
    }
}
