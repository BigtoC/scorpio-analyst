use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use chrono::Utc;
use scorpio_core::state::TradingState;
use scorpio_reporters::{ReportContext, Reporter, ReporterChain};

struct OkReporter;
struct FailReporter;

#[async_trait]
impl Reporter for OkReporter {
    fn name(&self) -> &'static str {
        "ok"
    }
    async fn emit(&self, _: Arc<TradingState>, _: Arc<ReportContext>) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl Reporter for FailReporter {
    fn name(&self) -> &'static str {
        "fail"
    }
    async fn emit(&self, _: Arc<TradingState>, _: Arc<ReportContext>) -> anyhow::Result<()> {
        anyhow::bail!("intentional test failure")
    }
}

struct CountingReporter(Arc<AtomicUsize>);

#[async_trait]
impl Reporter for CountingReporter {
    fn name(&self) -> &'static str {
        "counting"
    }
    async fn emit(&self, _: Arc<TradingState>, _: Arc<ReportContext>) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn test_state() -> Arc<TradingState> {
    Arc::new(TradingState::new("AAPL", "2026-04-23"))
}

fn test_ctx() -> Arc<ReportContext> {
    Arc::new(ReportContext {
        symbol: "AAPL".to_owned(),
        finished_at: Utc::now(),
        output_dir: std::path::PathBuf::from("/tmp"),
    })
}

#[tokio::test]
async fn run_all_returns_zero_when_all_reporters_succeed() {
    let mut chain = ReporterChain::new();
    chain.push(OkReporter);
    chain.push(OkReporter);
    let failures = chain.run_all(test_state(), test_ctx()).await;
    assert_eq!(failures, 0);
}

#[tokio::test]
async fn run_all_counts_failed_reporters() {
    let mut chain = ReporterChain::new();
    chain.push(OkReporter);
    chain.push(FailReporter);
    chain.push(OkReporter);
    let failures = chain.run_all(test_state(), test_ctx()).await;
    assert_eq!(failures, 1);
}

#[tokio::test]
async fn run_all_continues_past_failure_remaining_reporters_execute() {
    let counter = Arc::new(AtomicUsize::new(0));
    let mut chain = ReporterChain::new();
    chain.push(CountingReporter(Arc::clone(&counter)));
    chain.push(FailReporter);
    chain.push(CountingReporter(Arc::clone(&counter)));
    let failures = chain.run_all(test_state(), test_ctx()).await;
    assert_eq!(failures, 1);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "both ok reporters must have run despite the failure"
    );
}

#[tokio::test]
async fn run_all_returns_full_count_when_every_reporter_fails() {
    let mut chain = ReporterChain::new();
    chain.push(FailReporter);
    chain.push(FailReporter);
    let n = chain.len();
    let failures = chain.run_all(test_state(), test_ctx()).await;
    assert_eq!(failures, n);
}

#[tokio::test]
async fn run_all_returns_zero_for_empty_chain() {
    let chain = ReporterChain::new();
    let failures = chain.run_all(test_state(), test_ctx()).await;
    assert_eq!(failures, 0);
}

#[test]
fn len_reflects_pushed_reporters() {
    let mut chain = ReporterChain::new();
    assert_eq!(chain.len(), 0);
    assert!(chain.is_empty());
    chain.push(OkReporter);
    assert_eq!(chain.len(), 1);
    assert!(!chain.is_empty());
    chain.push(FailReporter);
    assert_eq!(chain.len(), 2);
}
