use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scorpio_core::state::TradingState;

pub mod json;
pub mod terminal;

/// Per-run metadata passed to every reporter at emit time.
#[derive(Debug, Clone)]
pub struct ReportContext {
    pub symbol: String,
    pub finished_at: DateTime<Utc>,
    /// Directory where file reporters write. Created on demand.
    pub output_dir: PathBuf,
}

/// Output leg for a completed analysis run.
///
/// Implementations must be `Send + Sync + 'static` so they can be moved into
/// independent `tokio::spawn` tasks by [`ReporterChain::run_all`].
#[async_trait]
pub trait Reporter: Send + Sync + 'static {
    /// Stable identifier used in logs and error messages (e.g. `"terminal"`, `"json"`).
    fn name(&self) -> &'static str;

    /// Emit a report for the completed analysis run.
    async fn emit(&self, state: Arc<TradingState>, ctx: Arc<ReportContext>) -> anyhow::Result<()>;
}

/// Ordered collection of reporters executed concurrently on [`run_all`].
pub struct ReporterChain {
    reporters: Vec<Box<dyn Reporter>>,
}

impl ReporterChain {
    pub fn new() -> Self {
        Self {
            reporters: Vec::new(),
        }
    }

    pub fn push<R: Reporter + 'static>(&mut self, r: R) {
        self.reporters.push(Box::new(r));
    }

    pub fn len(&self) -> usize {
        self.reporters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reporters.is_empty()
    }

    /// Spawn each reporter as an independent `tokio::spawn` task (true
    /// OS-thread parallelism). Fail-soft: a panicking or failing reporter is
    /// logged as a warning; the remaining tasks are unaffected. Returns the
    /// count of failed reporters.
    pub async fn run_all(self, state: Arc<TradingState>, ctx: Arc<ReportContext>) -> usize {
        let handles: Vec<_> = self
            .reporters
            .into_iter()
            .map(|r| {
                let state = Arc::clone(&state);
                let ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    let name = r.name();
                    (name, r.emit(state, ctx).await)
                })
            })
            .collect();

        let mut failures = 0;
        for handle in handles {
            match handle.await {
                Ok((_, Ok(()))) => {}
                Ok((name, Err(e))) => {
                    tracing::warn!(reporter = name, error = %e, "reporter failed");
                    failures += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "reporter task panicked");
                    failures += 1;
                }
            }
        }
        failures
    }
}

impl Default for ReporterChain {
    fn default() -> Self {
        Self::new()
    }
}
