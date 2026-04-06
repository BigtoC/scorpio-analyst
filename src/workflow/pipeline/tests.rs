use std::sync::Arc;

use graph_flow::Task;

use super::{constants::TASKS, errors::map_graph_error, runtime::canonicalize_runtime_symbol};
use crate::{error::TradingError, workflow::SnapshotStore};

#[test]
fn map_graph_error_extracts_task_phase_from_task_execution_failure() {
    let err = map_graph_error(graph_flow::GraphError::TaskExecutionFailed(
        "Task 'bullish_researcher' failed: provider timeout".to_owned(),
    ));

    match err {
        TradingError::GraphFlow { phase, task, cause } => {
            assert_eq!(phase, "researcher_debate");
            assert_eq!(task, "bullish_researcher");
            assert_eq!(cause, "provider timeout");
        }
        other => panic!("expected GraphFlow error, got: {other:?}"),
    }
}

#[test]
fn map_graph_error_extracts_fanout_child_identity() {
    let err = map_graph_error(graph_flow::GraphError::TaskExecutionFailed(
        "FanOut child 'technical_analyst' failed: bad response".to_owned(),
    ));

    match err {
        TradingError::GraphFlow { phase, task, cause } => {
            assert_eq!(phase, "analyst_team");
            assert_eq!(task, "technical_analyst");
            assert_eq!(cause, "bad response");
        }
        other => panic!("expected GraphFlow error, got: {other:?}"),
    }
}

#[test]
fn canonicalizes_runtime_symbol_before_prefetch() {
    let canonical = canonicalize_runtime_symbol(" nvda ").expect("valid lowercase symbol");
    assert_eq!(canonical, "NVDA");
}

#[test]
fn rejects_invalid_runtime_symbol_before_prefetch() {
    let err = canonicalize_runtime_symbol("DROP;TABLE").expect_err("invalid symbol must fail");
    assert!(matches!(err, TradingError::SchemaViolation { .. }));
}

#[tokio::test]
async fn task_id_constants_match_task_impl_ids() {
    let config = Arc::new(crate::config::Config {
        llm: crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: crate::config::TradingConfig {
            asset_symbol: "AAPL".to_owned(),
            backtest_start: None,
            backtest_end: None,
        },
        api: Default::default(),
        providers: Default::default(),
        storage: Default::default(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
    });
    let snapshot_store = Arc::new(SnapshotStore::new(None).await.expect("snapshot store"));
    let finnhub = crate::data::FinnhubClient::for_test();
    let handle = crate::providers::factory::CompletionModelHandle::for_test();

    assert_eq!(
        TASKS.preflight,
        crate::workflow::tasks::PreflightTask::new(Default::default()).id()
    );
    assert_eq!(
        TASKS.analyst_fan_out,
        graph_flow::fanout::FanOutTask::new(
            TASKS.analyst_fan_out,
            vec![crate::workflow::tasks::FundamentalAnalystTask::new(
                handle.clone(),
                finnhub.clone(),
                config.llm.clone(),
            )],
        )
        .id()
    );
    assert_eq!(
        TASKS.analyst_sync,
        crate::workflow::tasks::AnalystSyncTask::new(Arc::clone(&snapshot_store)).id()
    );
    assert_eq!(
        TASKS.bullish_researcher,
        crate::workflow::tasks::BullishResearcherTask::new(Arc::clone(&config), handle.clone())
            .id()
    );
    assert_eq!(
        TASKS.bearish_researcher,
        crate::workflow::tasks::BearishResearcherTask::new(Arc::clone(&config), handle.clone())
            .id()
    );
    assert_eq!(
        TASKS.debate_moderator,
        crate::workflow::tasks::DebateModeratorTask::new(
            Arc::clone(&config),
            handle.clone(),
            Arc::clone(&snapshot_store),
        )
        .id()
    );
    assert_eq!(
        TASKS.trader,
        crate::workflow::tasks::TraderTask::new(Arc::clone(&config), Arc::clone(&snapshot_store))
            .id()
    );
    assert_eq!(
        TASKS.aggressive_risk,
        crate::workflow::tasks::AggressiveRiskTask::new(Arc::clone(&config), handle.clone()).id()
    );
    assert_eq!(
        TASKS.conservative_risk,
        crate::workflow::tasks::ConservativeRiskTask::new(Arc::clone(&config), handle.clone()).id()
    );
    assert_eq!(
        TASKS.neutral_risk,
        crate::workflow::tasks::NeutralRiskTask::new(Arc::clone(&config), handle.clone()).id()
    );
    assert_eq!(
        TASKS.risk_moderator,
        crate::workflow::tasks::RiskModeratorTask::new(
            Arc::clone(&config),
            handle.clone(),
            Arc::clone(&snapshot_store),
        )
        .id()
    );
    assert_eq!(
        TASKS.fund_manager,
        crate::workflow::tasks::FundManagerTask::new(config, snapshot_store).id()
    );
}
