//! Hermetic integration tests for [`scorpio_core::app::AnalysisRuntime`].
//!
//! The `from_pipeline` constructor is gated on the `test-helpers` feature so
//! we can wrap a stubbed-task pipeline (built via `workflow::test_support`)
//! without having to go through the real provider/model assembly path inside
//! `AnalysisRuntime::new`.

#![cfg(feature = "test-helpers")]

#[path = "support/workflow_pipeline_make_pipeline.rs"]
mod workflow_pipeline_make_pipeline;

use scorpio_core::app::AnalysisRuntime;
use scorpio_core::config::{
    ApiConfig, Config, DataEnrichmentConfig, LlmConfig, ProvidersConfig, RateLimitConfig,
    StorageConfig, TradingConfig,
};
use scorpio_core::state::TradingState;
use scorpio_core::workflow::SnapshotStore;
use scorpio_core::workflow::test_support::{
    deserialize_state_from_context, serialize_state_to_context,
};
use std::sync::Arc;

struct NoFinalStatusFundManagerTask {
    snapshot_store: Arc<SnapshotStore>,
}

#[async_trait::async_trait]
impl graph_flow::Task for NoFinalStatusFundManagerTask {
    fn id(&self) -> &str {
        "fund_manager"
    }

    async fn run(
        &self,
        context: graph_flow::Context,
    ) -> graph_flow::Result<graph_flow::TaskResult> {
        let state: TradingState =
            deserialize_state_from_context(&context)
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "NoFinalStatusFundManagerTask: state deser failed: {error}"
                    ))
                })?;

        serialize_state_to_context(&state, &context)
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NoFinalStatusFundManagerTask: state ser failed: {error}"
                ))
            })?;

        let execution_id = state.execution_id.to_string();
        self.snapshot_store
            .save_snapshot(
                &execution_id,
                scorpio_core::workflow::SnapshotPhase::FundManager,
                &state,
                None,
            )
            .await
            .map_err(|error| {
                graph_flow::GraphError::TaskExecutionFailed(format!(
                    "NoFinalStatusFundManagerTask: snapshot save failed: {error}"
                ))
            })?;

        Ok(graph_flow::TaskResult::new(
            None,
            graph_flow::NextAction::End,
        ))
    }
}

#[tokio::test]
async fn run_returns_state_with_final_execution_status() {
    let (pipeline, _verify_store, _dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "app-runtime-success.db",
        "app-runtime",
        0,
        0,
    )
    .await;

    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");

    let runtime = AnalysisRuntime::from_pipeline(pipeline);
    let state = runtime
        .run("AAPL")
        .await
        .expect("stubbed analysis cycle should succeed");

    assert!(
        state.final_execution_status.is_some(),
        "AnalysisRuntime::run must surface a final_execution_status"
    );
    assert_eq!(state.asset_symbol, "AAPL");
}

#[tokio::test]
async fn run_rejects_invalid_symbol_before_executing_pipeline() {
    let (pipeline, _verify_store, _dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "app-runtime-reject.db",
        "app-runtime",
        0,
        0,
    )
    .await;

    // Stub tasks intentionally NOT installed: the symbol guard must trip
    // before any pipeline task is scheduled.
    let runtime = AnalysisRuntime::from_pipeline(pipeline);
    let err = runtime
        .run("NOT A VALID SYMBOL!!")
        .await
        .expect_err("invalid symbol must fail validation");

    assert!(
        format!("{err:#}").contains("invalid symbol"),
        "symbol validation should surface the original schema-violation message; got {err:#}"
    );
}

#[tokio::test]
async fn run_errors_when_pipeline_finishes_without_final_execution_status() {
    let (pipeline, verify_store, _dir) = workflow_pipeline_make_pipeline::make_pipeline(
        "app-runtime-no-final-status.db",
        "app-runtime",
        0,
        0,
    )
    .await;

    pipeline
        .install_stub_tasks_for_test()
        .expect("stub install must succeed");
    pipeline
        .replace_task_for_test(Arc::new(NoFinalStatusFundManagerTask {
            snapshot_store: verify_store,
        }))
        .expect("fund manager test seam must install");

    let runtime = AnalysisRuntime::from_pipeline(pipeline);
    let err = runtime
        .run("AAPL")
        .await
        .expect_err("missing final execution status should fail");

    assert!(
        format!("{err:#}").contains("pipeline completed without a final execution status"),
        "runtime should preserve the facade guard message; got {err:#}"
    );
}

#[tokio::test]
async fn new_wraps_snapshot_store_initialization_failures() {
    let cfg = Config {
        llm: LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 0,
            max_risk_rounds: 0,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: TradingConfig::default(),
        api: ApiConfig::default(),
        providers: ProvidersConfig::default(),
        storage: StorageConfig {
            snapshot_db_path: "/dev/null/scorpio-phase-snapshots.db".to_owned(),
        },
        rate_limits: RateLimitConfig::default(),
        enrichment: DataEnrichmentConfig::default(),
        analysis_pack: "baseline".to_owned(),
    };

    let err = AnalysisRuntime::new(cfg)
        .await
        .expect_err("invalid snapshot path should fail runtime assembly");

    assert!(
        format!("{err:#}").contains("failed to initialize snapshot storage"),
        "runtime should preserve snapshot-store context; got {err:#}"
    );
}
