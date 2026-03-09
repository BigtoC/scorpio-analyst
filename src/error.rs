use std::time::Duration;

use thiserror::Error;

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
    NetworkTimeout {
        elapsed: Duration,
        message: String,
    },

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
    /// Calculate the delay for a given attempt (0-indexed), using exponential backoff.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        self.base_delay * 2u32.saturating_pow(attempt)
    }
}

/// Result of an analyst fan-out where some agents may have failed.
///
/// Degradation rules:
/// - 1 failure: continue with partial data
/// - 2+ failures: abort the cycle
pub fn check_analyst_degradation(
    total: usize,
    failures: usize,
) -> Result<(), TradingError> {
    if failures >= 2 || (total > 0 && failures == total) {
        return Err(TradingError::AnalystError {
            agent: "fan-out".to_owned(),
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
        assert!(check_analyst_degradation(4, 1).is_ok());
    }

    #[test]
    fn degradation_aborts_on_two_failures() {
        assert!(check_analyst_degradation(4, 2).is_err());
    }

    #[test]
    fn degradation_aborts_on_total_failure() {
        assert!(check_analyst_degradation(4, 4).is_err());
    }
}
