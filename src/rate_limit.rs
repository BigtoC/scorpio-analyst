use std::num::NonZeroU32;
use std::sync::Arc;

use governor::{Quota, RateLimiter};

/// A shared, async-aware rate limiter backed by `governor`.
///
/// Wrap in `Arc` and inject into data clients and agent tasks to
/// enforce per-provider request quotas across concurrent operations.
#[derive(Debug, Clone)]
pub struct SharedRateLimiter {
    inner: Arc<RateLimiter<governor::state::NotKeyed, governor::state::InMemoryState, governor::clock::DefaultClock>>,
    label: String,
}

impl SharedRateLimiter {
    /// Create a new rate limiter allowing `per_second` requests per second.
    ///
    /// # Panics
    /// Panics if `per_second` is 0.
    pub fn new(label: impl Into<String>, per_second: u32) -> Self {
        let nz = NonZeroU32::new(per_second).expect("per_second must be > 0");
        let quota = Quota::per_second(nz);
        Self {
            inner: Arc::new(RateLimiter::direct(quota)),
            label: label.into(),
        }
    }

    /// Wait until a single permit becomes available. This is cancel-safe.
    pub async fn acquire(&self) {
        self.inner.until_ready().await;
    }

    /// The human-readable label for this limiter (e.g., provider name).
    pub fn label(&self) -> &str {
        &self.label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_issues_permits() {
        let limiter = SharedRateLimiter::new("test", 100);
        // Should not block for a single permit
        limiter.acquire().await;
        assert_eq!(limiter.label(), "test");
    }
}
