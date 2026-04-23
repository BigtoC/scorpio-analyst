use std::sync::Arc;

use graph_flow::{Context, NextAction, Task, TaskResult, fanout::FanOutTask};

use super::{constants::TASKS, errors::map_graph_error, runtime::canonicalize_runtime_symbol};
use crate::{
    error::TradingError,
    state::{
        AgentTokenUsage, DataCoverageReport, EvidenceKind, EvidenceRecord, EvidenceSource,
        FundamentalData, NewsData, ProvenanceSummary, SentimentData, TradingState,
    },
    workflow::{
        SnapshotStore, context_bridge::write_prefixed_result,
        tasks::test_helpers::replace_with_stubs,
    },
};

const ANALYST_PREFIX: &str = "analyst";
const ANALYST_FUNDAMENTAL: &str = "fundamental";
const ANALYST_SENTIMENT: &str = "sentiment";
const ANALYST_NEWS: &str = "news";
const ANALYST_TECHNICAL: &str = "technical";

struct PartialAnalystChild {
    analyst_key: &'static str,
    task_id: &'static str,
}

impl PartialAnalystChild {
    fn fundamental() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_FUNDAMENTAL,
            task_id: "fundamental_analyst",
        })
    }

    fn sentiment() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_SENTIMENT,
            task_id: "sentiment_analyst",
        })
    }

    fn news() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_NEWS,
            task_id: "news_analyst",
        })
    }

    fn technical_missing() -> Arc<Self> {
        Arc::new(Self {
            analyst_key: ANALYST_TECHNICAL,
            task_id: "technical_analyst",
        })
    }
}

#[async_trait::async_trait]
impl Task for PartialAnalystChild {
    fn id(&self) -> &str {
        self.task_id
    }

    async fn run(&self, context: Context) -> graph_flow::Result<TaskResult> {
        match self.analyst_key {
            ANALYST_FUNDAMENTAL => {
                write_prefixed_result(
                    &context,
                    ANALYST_PREFIX,
                    ANALYST_FUNDAMENTAL,
                    &FundamentalData {
                        revenue_growth_pct: Some(12.5),
                        pe_ratio: Some(24.5),
                        eps: Some(6.05),
                        current_ratio: None,
                        debt_to_equity: None,
                        gross_margin: None,
                        net_income: None,
                        insider_transactions: vec![],
                        summary: "stub: strong fundamentals".to_owned(),
                    },
                )
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PartialAnalystChild(fundamental): context write failed: {error}"
                    ))
                })?;
            }
            ANALYST_SENTIMENT => {
                write_prefixed_result(
                    &context,
                    ANALYST_PREFIX,
                    ANALYST_SENTIMENT,
                    &SentimentData {
                        overall_score: 0.72,
                        source_breakdown: vec![],
                        engagement_peaks: vec![],
                        summary: "stub: positive sentiment".to_owned(),
                    },
                )
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PartialAnalystChild(sentiment): context write failed: {error}"
                    ))
                })?;
            }
            ANALYST_NEWS => {
                write_prefixed_result(
                    &context,
                    ANALYST_PREFIX,
                    ANALYST_NEWS,
                    &NewsData {
                        articles: vec![],
                        macro_events: vec![],
                        summary: "stub: no major news".to_owned(),
                    },
                )
                .await
                .map_err(|error| {
                    graph_flow::GraphError::TaskExecutionFailed(format!(
                        "PartialAnalystChild(news): context write failed: {error}"
                    ))
                })?;
            }
            ANALYST_TECHNICAL => {}
            other => {
                return Err(graph_flow::GraphError::TaskExecutionFailed(format!(
                    "PartialAnalystChild: unknown analyst key '{other}'"
                )));
            }
        }

        let ok = self.analyst_key != ANALYST_TECHNICAL;
        context
            .set(format!("analyst.{}.ok", self.analyst_key), ok)
            .await;
        if !ok {
            context
                .set(
                    format!("analyst.{}.err", self.analyst_key),
                    "stub: technical omitted".to_owned(),
                )
                .await;
        }

        write_prefixed_result(
            &context,
            "usage.analyst",
            self.analyst_key,
            &AgentTokenUsage::unavailable("Stub Analyst", "stub-model", 0),
        )
        .await
        .map_err(|error| {
            graph_flow::GraphError::TaskExecutionFailed(format!(
                "PartialAnalystChild({}): usage write failed: {error}",
                self.analyst_key
            ))
        })?;

        Ok(TaskResult::new(None, NextAction::Continue))
    }
}

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

#[test]
fn config_loads_default_valuation_fetch_timeout_secs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
quick_thinking_provider = "openai"
deep_thinking_provider = "openai"
quick_thinking_model = "gpt-4o-mini"
deep_thinking_model = "o3"
"#,
    )
    .expect("write config");
    let cfg = crate::config::Config::load_from(&path).expect("config should load");

    assert_eq!(cfg.llm.valuation_fetch_timeout_secs, 30);
}

async fn test_snapshot_store(db_name: &str) -> (SnapshotStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join(db_name);
    let store = SnapshotStore::new(Some(&db_path))
        .await
        .expect("snapshot store");
    (store, dir)
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
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: crate::config::TradingConfig::default(),
        api: Default::default(),
        providers: Default::default(),
        storage: Default::default(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
        analysis_pack: "baseline".to_owned(),
    });
    let (snapshot_store, _dir) = test_snapshot_store("task-id-constants.db").await;
    let snapshot_store = Arc::new(snapshot_store);
    let finnhub = crate::data::FinnhubClient::for_test();
    let handle = crate::providers::factory::CompletionModelHandle::for_test();

    assert_eq!(
        TASKS.preflight,
        crate::workflow::tasks::PreflightTask::new(Default::default(), snapshot_store.clone()).id()
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

#[tokio::test]
async fn run_analysis_cycle_clears_stale_evidence_and_reporting_fields_from_reused_state() {
    let config = crate::config::Config {
        llm: crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 1,
        },
        trading: crate::config::TradingConfig::default(),
        api: Default::default(),
        providers: Default::default(),
        storage: Default::default(),
        rate_limits: Default::default(),
        enrichment: Default::default(),
        analysis_pack: "baseline".to_owned(),
    };
    let (snapshot_store, _dir) = test_snapshot_store("pipeline-reused-state.db").await;
    let pipeline = crate::workflow::TradingPipeline::new(
        config,
        crate::data::FinnhubClient::for_test(),
        crate::data::FredClient::for_test(),
        crate::data::YFinanceClient::new(crate::rate_limit::SharedRateLimiter::new(
            "pipeline-test",
            10,
        )),
        snapshot_store,
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::providers::factory::CompletionModelHandle::for_test(),
    );
    replace_with_stubs(&pipeline, Arc::clone(&pipeline.snapshot_store))
        .expect("stub install must succeed");
    pipeline
        .replace_task_for_test(FanOutTask::new(
            TASKS.analyst_fan_out,
            vec![
                PartialAnalystChild::fundamental(),
                PartialAnalystChild::sentiment(),
                PartialAnalystChild::news(),
                PartialAnalystChild::technical_missing(),
            ],
        ))
        .expect("fanout replacement must succeed");

    let mut initial_state = TradingState::new("AAPL", "2026-03-20");
    initial_state.evidence_fundamental = Some(EvidenceRecord {
        kind: EvidenceKind::Fundamental,
        payload: FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(999.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "stale fundamentals".to_owned(),
        },
        sources: vec![EvidenceSource {
            provider: "stale-provider".to_owned(),
            datasets: vec!["stale-dataset".to_owned()],
            fetched_at: chrono::Utc::now(),
            effective_at: None,
            url: None,
            citation: None,
        }],
        quality_flags: vec![],
    });
    initial_state.evidence_technical =
        initial_state
            .evidence_fundamental
            .clone()
            .map(|record| EvidenceRecord {
                kind: EvidenceKind::Technical,
                payload: crate::state::TechnicalData {
                    rsi: Some(1.0),
                    macd: None,
                    atr: None,
                    sma_20: None,
                    sma_50: None,
                    ema_12: None,
                    ema_26: None,
                    bollinger_upper: None,
                    bollinger_lower: None,
                    support_level: None,
                    resistance_level: None,
                    volume_avg: None,
                    summary: record.payload.summary,
                },
                sources: record.sources,
                quality_flags: record.quality_flags,
            });
    initial_state.data_coverage = Some(DataCoverageReport {
        required_inputs: vec!["stale".to_owned()],
        missing_inputs: vec![],
    });
    initial_state.provenance_summary = Some(ProvenanceSummary {
        providers_used: vec!["stale-provider".to_owned()],
    });

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must succeed with one missing analyst input");

    assert!(final_state.evidence_technical.is_none());
    assert_eq!(
        final_state
            .data_coverage
            .as_ref()
            .expect("coverage must be recomputed")
            .missing_inputs,
        vec!["technical"]
    );
    assert_eq!(
        final_state
            .provenance_summary
            .as_ref()
            .expect("provenance must be recomputed")
            .providers_used,
        vec!["finnhub", "fred"]
    );
}
