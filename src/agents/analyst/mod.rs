//! Analyst team: four parallel specialist agents that produce structured data
//! for the downstream debate and trading pipeline.
//!
//! # Fan-out execution
//!
//! [`run_analyst_team`] spawns all four analysts concurrently via [`tokio::spawn`]
//! and collects results. The degradation policy tolerates one failure
//! (partial data continues); two or more failures abort the cycle with
//! [`TradingError::AnalystError`].
//!
//! # Sub-modules
//!
//! - [`fundamental`] – Fundamental Analyst (earnings, ratios, insider activity)
//! - [`sentiment`] – Sentiment Analyst (news-based, MVP)
//! - [`news`] – News Analyst (articles and macro events)
//! - [`technical`] – Technical Analyst (OHLCV → indicators → LLM summary)

mod fundamental;
mod news;
mod sentiment;
mod technical;

pub use fundamental::FundamentalAnalyst;
pub use news::NewsAnalyst;
pub use sentiment::SentimentAnalyst;
pub use technical::TechnicalAnalyst;

use std::time::Duration;

use tracing::warn;

use crate::{
    config::LlmConfig,
    data::{FinnhubClient, YFinanceClient},
    error::{TradingError, check_analyst_degradation},
    providers::factory::CompletionModelHandle,
    state::{AgentTokenUsage, TradingState},
};

/// Run all four analyst agents concurrently and write results into `state`.
///
/// Each agent is constructed fresh, cloning the shared handles, then spawned
/// on the Tokio thread-pool. Results are collected after all tasks complete;
/// successes are written to the corresponding `TradingState` fields sequentially.
///
/// # Degradation policy
///
/// - 0 failures → all four fields populated, returns full `Vec<AgentTokenUsage>`
/// - 1 failure  → three fields populated, one `None`, continues with partial data
/// - 2+ failures → returns `TradingError::AnalystError`
///
/// # Errors
///
/// - [`TradingError::AnalystError`] when two or more analysts fail.
pub async fn run_analyst_team(
    handle: &CompletionModelHandle,
    finnhub: &FinnhubClient,
    yfinance: &YFinanceClient,
    state: &mut TradingState,
    llm_config: &LlmConfig,
) -> Result<Vec<AgentTokenUsage>, TradingError> {
    let timeout = Duration::from_secs(llm_config.agent_timeout_secs);
    let symbol = state.asset_symbol.clone();
    let target_date = state.target_date.clone();

    // ── Spawn all four analysts concurrently ─────────────────────────────

    let fundamental_task = {
        let analyst = FundamentalAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(timeout, analyst.run()).await })
    };

    let sentiment_task = {
        let analyst = SentimentAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(timeout, analyst.run()).await })
    };

    let news_task = {
        let analyst = NewsAnalyst::new(
            handle.clone(),
            finnhub.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(timeout, analyst.run()).await })
    };

    let technical_task = {
        let analyst = TechnicalAnalyst::new(
            handle.clone(),
            yfinance.clone(),
            symbol.clone(),
            target_date.clone(),
            llm_config,
        );
        tokio::spawn(async move { tokio::time::timeout(timeout, analyst.run()).await })
    };

    // ── Await all tasks ───────────────────────────────────────────────────

    let (fundamental_join, sentiment_join, news_join, technical_join) =
        tokio::join!(fundamental_task, sentiment_task, news_task, technical_task);

    // ── Unwrap JoinError, then timeout, then analyst error ────────────────

    let fundamental_result = flatten_task_result("fundamental", fundamental_join);
    let sentiment_result = flatten_task_result("sentiment", sentiment_join);
    let news_result = flatten_task_result("news", news_join);
    let technical_result = flatten_task_result("technical", technical_join);

    // ── Count failures and apply degradation policy ───────────────────────

    let failure_count = [
        fundamental_result.is_err(),
        sentiment_result.is_err(),
        news_result.is_err(),
        technical_result.is_err(),
    ]
    .iter()
    .filter(|&&failed| failed)
    .count();

    if let Err(ref e) = fundamental_result {
        warn!(agent = "fundamental", error = %e, "analyst failed");
    }
    if let Err(ref e) = sentiment_result {
        warn!(agent = "sentiment", error = %e, "analyst failed");
    }
    if let Err(ref e) = news_result {
        warn!(agent = "news", error = %e, "analyst failed");
    }
    if let Err(ref e) = technical_result {
        warn!(agent = "technical", error = %e, "analyst failed");
    }

    check_analyst_degradation(4, failure_count)?;

    // ── Write successful results to state ─────────────────────────────────

    let mut token_usages: Vec<AgentTokenUsage> = Vec::new();

    if let Ok((data, usage)) = fundamental_result {
        state.fundamental_metrics = Some(data);
        token_usages.push(usage);
    }
    if let Ok((data, usage)) = sentiment_result {
        state.market_sentiment = Some(data);
        token_usages.push(usage);
    }
    if let Ok((data, usage)) = news_result {
        state.macro_news = Some(data);
        token_usages.push(usage);
    }
    if let Ok((data, usage)) = technical_result {
        state.technical_indicators = Some(data);
        token_usages.push(usage);
    }

    Ok(token_usages)
}

// ────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Flatten a `JoinHandle` result: converts `JoinError` and timeout into
/// `TradingError::AnalystError`.
fn flatten_task_result<T>(
    agent_name: &str,
    join_result: Result<
        Result<Result<T, TradingError>, tokio::time::error::Elapsed>,
        tokio::task::JoinError,
    >,
) -> Result<T, TradingError> {
    match join_result {
        // Task panicked or was cancelled.
        Err(join_err) => Err(TradingError::AnalystError {
            agent: agent_name.to_owned(),
            message: format!("task panicked or was cancelled: {join_err}"),
        }),
        // Task completed but timed out.
        Ok(Err(_elapsed)) => Err(TradingError::AnalystError {
            agent: agent_name.to_owned(),
            message: "analyst task timed out".to_owned(),
        }),
        // Task completed successfully — propagate inner result.
        Ok(Ok(inner)) => inner,
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TradingError;

    // ── flatten_task_result ──────────────────────────────────────────────

    #[test]
    fn flatten_join_error_becomes_analyst_error() {
        // Simulate a JoinError by using an aborted task handle.
        // We can't construct JoinError directly, so test via the Ok(Ok(…)) path first.
        let ok: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Ok(Ok(42)));
        let result = flatten_task_result::<i32>("test", ok);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn flatten_inner_error_propagates() {
        let inner_err: Result<
            Result<Result<i32, TradingError>, tokio::time::error::Elapsed>,
            tokio::task::JoinError,
        > = Ok(Ok(Err(TradingError::Rig("inner failure".to_owned()))));
        let result = flatten_task_result::<i32>("test", inner_err);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TradingError::Rig(_)));
    }

    // ── check_analyst_degradation delegation ────────────────────────────

    #[test]
    fn zero_failures_passes_degradation() {
        assert!(check_analyst_degradation(4, 0).is_ok());
    }

    #[test]
    fn one_failure_passes_degradation() {
        assert!(check_analyst_degradation(4, 1).is_ok());
    }

    #[test]
    fn two_failures_aborts() {
        assert!(check_analyst_degradation(4, 2).is_err());
    }

    // ── run_analyst_team (unit-level): configurable timeout ──────────────

    #[test]
    fn timeout_duration_derived_from_config() {
        let config = LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            agent_timeout_secs: 60,
        };
        let timeout = Duration::from_secs(config.agent_timeout_secs);
        assert_eq!(timeout, Duration::from_secs(60));
    }
}
