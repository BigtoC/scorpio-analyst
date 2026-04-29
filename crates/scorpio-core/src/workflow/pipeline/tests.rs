use std::sync::Arc;

use graph_flow::{Context, NextAction, Task, TaskResult, fanout::FanOutTask};

use super::{constants::TASKS, errors::map_graph_error, runtime::canonicalize_runtime_symbol};
use crate::{
    error::TradingError,
    state::{
        AgentTokenUsage, DataCoverageReport, EvidenceKind, EvidenceRecord, EvidenceSource,
        FundamentalData, NewsData, ProvenanceSummary, SentimentData, TechnicalData,
        TechnicalOptionsContext, TradingState,
    },
    workflow::{
        SnapshotStore, context_bridge::write_prefixed_result,
        tasks::test_helpers::{replace_with_stubs, replace_with_stubs_using_technical},
    },
};

// ─── Helpers shared by hydration tests ───────────────────────────────────────

#[cfg(test)]
fn to_money_usd(v: f64) -> paft_money::Money {
    use paft_money::{Currency, IsoCurrency, Money};
    let d = rust_decimal::Decimal::try_from(v).unwrap();
    Money::new(d, Currency::Iso(IsoCurrency::USD)).unwrap()
}

#[cfg(test)]
fn make_trend_row_for_test(
    eps_avg: Option<f64>,
    revenue_avg: Option<f64>,
    num_analysts: Option<u32>,
) -> yfinance_rs::analysis::EarningsTrendRow {
    let to_money = to_money_usd;
    let json = serde_json::json!({
        "period": "0Q",
        "growth": null,
        "earnings_estimate": {
            "avg": eps_avg.map(&to_money),
            "low": null,
            "high": null,
            "year_ago_eps": null,
            "num_analysts": num_analysts,
            "growth": null
        },
        "revenue_estimate": {
            "avg": revenue_avg.map(&to_money),
            "low": null,
            "high": null,
            "year_ago_revenue": null,
            "num_analysts": null,
            "growth": null
        },
        "eps_trend": { "current": null, "historical": [] },
        "eps_revisions": { "historical": [] }
    });
    serde_json::from_value(json).expect("valid test EarningsTrendRow")
}

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
    assert_eq!(canonical.to_string(), "NVDA");
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
    initial_state.set_evidence_fundamental(EvidenceRecord {
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
    if let Some(fund) = initial_state.evidence_fundamental().cloned() {
        initial_state.set_evidence_technical(EvidenceRecord {
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
                summary: fund.payload.summary,
                options_summary: None,
                options_context: None,
            },
            sources: fund.sources,
            quality_flags: fund.quality_flags,
        });
    }
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

    assert!(final_state.evidence_technical().is_none());
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

#[tokio::test]
async fn try_new_rejects_invalid_pack_id_with_typed_error() {
    // Construction-time pack-resolution failure surfaces as TradingError::Config
    // before any graph-build work runs. Production callers go through this
    // path so an invalid `config.analysis_pack` value never reaches preflight.
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
        analysis_pack: "totally-not-a-real-pack".to_owned(),
    };
    let (snapshot_store, _dir) = test_snapshot_store("pipeline-try-new-bad-pack.db").await;
    let result = crate::workflow::TradingPipeline::try_new(
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
    let err = result.expect_err("invalid pack id must surface as a typed error");
    let msg = format!("{err}");
    assert!(
        matches!(err, crate::error::TradingError::Config(_)),
        "expected TradingError::Config, got: {msg}"
    );
    assert!(
        msg.contains("totally-not-a-real-pack"),
        "error must name the offending pack id, got: {msg}"
    );
}

#[tokio::test]
async fn try_new_succeeds_for_baseline_pack_id() {
    // The happy path: construction with a valid pack id produces a usable
    // pipeline whose runtime_policy is hydrated to the resolved manifest.
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
    let (snapshot_store, _dir) = test_snapshot_store("pipeline-try-new-ok.db").await;
    let pipeline = crate::workflow::TradingPipeline::try_new(
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
    )
    .expect("baseline pack id must resolve");

    assert_eq!(
        pipeline.runtime_policy.as_ref().map(|p| p.pack_id),
        Some(crate::analysis_packs::PackId::Baseline)
    );
}

#[tokio::test]
async fn run_analysis_cycle_hydrates_extended_consensus_enrichment() {
    use yfinance_rs::analysis::{PriceTarget, RecommendationSummary};

    use crate::analysis_packs::{PackId, resolve_pack};
    use crate::data::StubbedFinancialResponses;
    use crate::workflow::builder::PipelineDeps;

    // Build a pack with consensus_estimates enabled.
    let mut pack = resolve_pack(PackId::Baseline);
    pack.enrichment_intent.consensus_estimates = true;

    let trend_rows = vec![make_trend_row_for_test(
        Some(2.15),
        Some(94_200_000_000.0),
        Some(28),
    )];

    let price_target = PriceTarget {
        mean: Some(to_money_usd(215.0)),
        high: Some(to_money_usd(265.0)),
        low: Some(to_money_usd(170.0)),
        number_of_analysts: Some(42),
    };

    let recommendation_summary = RecommendationSummary {
        strong_buy: Some(12),
        buy: Some(18),
        hold: Some(10),
        sell: Some(2),
        strong_sell: Some(0),
        ..RecommendationSummary::default()
    };

    let yfinance =
        crate::data::YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            trend: Some(trend_rows),
            price_target: Some(price_target),
            recommendation_summary: Some(recommendation_summary),
            ..StubbedFinancialResponses::default()
        });

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
    let (snapshot_store, _dir) =
        test_snapshot_store("pipeline-hydrate-extended-consensus.db").await;

    let pipeline = crate::workflow::TradingPipeline::from_pack(
        &pack,
        PipelineDeps {
            config,
            finnhub: crate::data::FinnhubClient::for_test(),
            fred: crate::data::FredClient::for_test(),
            yfinance,
            snapshot_store,
            quick_handle: crate::providers::factory::CompletionModelHandle::for_test(),
            deep_handle: crate::providers::factory::CompletionModelHandle::for_test(),
        },
    );

    replace_with_stubs(&pipeline, Arc::clone(&pipeline.snapshot_store))
        .expect("stub install must succeed");

    // Use today's date so hydrate_consensus passes the live-date gate.
    let target_date = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let initial_state = TradingState::new("AAPL", &target_date);

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must succeed with stubbed consensus enrichment");

    let consensus = final_state
        .enrichment_consensus
        .payload
        .as_ref()
        .expect("enrichment_consensus.payload must be populated by hydration");

    // Base fields from the trend row.
    let eps = consensus
        .eps_estimate
        .expect("eps_estimate must be present");
    assert!((eps - 2.15).abs() < 0.01, "expected eps ~2.15, got {eps}");
    assert_eq!(consensus.analyst_count, Some(28));

    // Price-target extended fields.
    let pt = consensus
        .price_target
        .as_ref()
        .expect("price_target must be populated from stub");
    assert!(
        matches!(pt.mean, Some(m) if (m - 215.0).abs() < 0.01),
        "price target mean must be ~215.0, got {:?}",
        pt.mean
    );
    assert!(
        matches!(pt.high, Some(h) if (h - 265.0).abs() < 0.01),
        "price target high must be ~265.0, got {:?}",
        pt.high
    );
    assert!(
        matches!(pt.low, Some(l) if (l - 170.0).abs() < 0.01),
        "price target low must be ~170.0, got {:?}",
        pt.low
    );
    assert_eq!(pt.analyst_count, Some(42));

    // Recommendation extended fields.
    let rec = consensus
        .recommendations
        .as_ref()
        .expect("recommendations must be populated from stub");
    assert_eq!(rec.strong_buy, Some(12));
    assert_eq!(rec.buy, Some(18));
    assert_eq!(rec.hold, Some(10));
    assert_eq!(rec.sell, Some(2));
    assert_eq!(rec.strong_sell, Some(0));
}

#[tokio::test]
async fn run_analysis_cycle_rehydrates_prior_consensus_counter_from_snapshot_store() {
    use crate::analysis_packs::{PackId, resolve_pack};
    use crate::data::StubbedFinancialResponses;
    use crate::workflow::builder::PipelineDeps;

    let mut pack = resolve_pack(PackId::Baseline);
    pack.enrichment_intent.consensus_estimates = true;

    let yfinance =
        crate::data::YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            trend: None,
            price_target_error: Some("price target down".to_owned()),
            recommendation_summary: None,
            ..StubbedFinancialResponses::default()
        });

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
    let (snapshot_store, _dir) =
        test_snapshot_store("pipeline-consensus-rehydrate-from-snapshot.db").await;

    let pipeline = crate::workflow::TradingPipeline::from_pack(
        &pack,
        PipelineDeps {
            config,
            finnhub: crate::data::FinnhubClient::for_test(),
            fred: crate::data::FredClient::for_test(),
            yfinance,
            snapshot_store,
            quick_handle: crate::providers::factory::CompletionModelHandle::for_test(),
            deep_handle: crate::providers::factory::CompletionModelHandle::for_test(),
        },
    );

    replace_with_stubs(&pipeline, Arc::clone(&pipeline.snapshot_store))
        .expect("stub install must succeed");

    let target_date = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    let first_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", &target_date))
        .await
        .expect("first degraded cycle must succeed fail-open");
    assert!(
        matches!(first_state.enrichment_consensus.status, crate::data::adapters::EnrichmentStatus::FetchFailed(ref reason) if reason == "provider_degraded"),
        "first cycle must classify as provider_degraded, got {:?}",
        first_state.enrichment_consensus.status
    );
    let first_payload = first_state
        .enrichment_consensus
        .payload
        .as_ref()
        .expect("first degraded cycle must persist a counter stub");
    assert_eq!(
        first_payload.consecutive_provider_degraded_cycles, 1,
        "first degraded cycle must persist counter=1"
    );

    let second_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", &target_date))
        .await
        .expect("fresh run must succeed fail-open");
    let second_payload = second_state
        .enrichment_consensus
        .payload
        .as_ref()
        .expect("fresh degraded cycle must persist a counter stub");

    assert_eq!(
        second_payload.consecutive_provider_degraded_cycles, 2,
        "fresh run must reload the prior phase-1 consensus payload instead of resetting the degraded counter"
    );
}

#[tokio::test]
async fn run_analysis_cycle_does_not_reuse_prior_consensus_payload_across_symbols() {
    use crate::analysis_packs::{PackId, resolve_pack};
    use crate::data::StubbedFinancialResponses;
    use crate::workflow::builder::PipelineDeps;

    let mut pack = resolve_pack(PackId::Baseline);
    pack.enrichment_intent.consensus_estimates = true;

    let yfinance =
        crate::data::YFinanceClient::with_stubbed_financials(StubbedFinancialResponses {
            trend: None,
            price_target_error: Some("price target down".to_owned()),
            recommendation_summary: None,
            ..StubbedFinancialResponses::default()
        });

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
    let (snapshot_store, _dir) =
        test_snapshot_store("pipeline-consensus-symbol-isolation.db").await;

    let pipeline = crate::workflow::TradingPipeline::from_pack(
        &pack,
        PipelineDeps {
            config,
            finnhub: crate::data::FinnhubClient::for_test(),
            fred: crate::data::FredClient::for_test(),
            yfinance,
            snapshot_store,
            quick_handle: crate::providers::factory::CompletionModelHandle::for_test(),
            deep_handle: crate::providers::factory::CompletionModelHandle::for_test(),
        },
    );

    replace_with_stubs(&pipeline, Arc::clone(&pipeline.snapshot_store))
        .expect("stub install must succeed");

    let target_date = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();

    let first_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", &target_date))
        .await
        .expect("first degraded cycle must succeed fail-open");
    let first_payload = first_state
        .enrichment_consensus
        .payload
        .as_ref()
        .expect("first degraded cycle must persist a counter stub");
    assert_eq!(
        first_payload.symbol, "AAPL",
        "precondition: first run must persist the original symbol"
    );
    assert_eq!(
        first_payload.consecutive_provider_degraded_cycles, 1,
        "precondition: first degraded cycle must persist counter=1"
    );

    let mut reused_state = first_state;
    reused_state.asset_symbol = "MSFT".to_owned();
    reused_state.symbol = None;

    let second_state = pipeline
        .run_analysis_cycle(reused_state)
        .await
        .expect("second degraded cycle must also succeed fail-open");
    let second_payload = second_state
        .enrichment_consensus
        .payload
        .as_ref()
        .expect("second degraded cycle must persist a counter stub");

    assert_eq!(
        second_payload.symbol, "MSFT",
        "reused state must not carry the prior symbol's consensus payload into the new run"
    );
    assert_eq!(
        second_payload.consecutive_provider_degraded_cycles, 1,
        "reused state for a different symbol must not inherit the prior symbol's degraded-cycle counter"
    );
}

// ─── Task 7: options_summary cleared on cycle reset ───────────────────────────

#[test]
fn clear_equity_resets_options_summary_unit() {
    let mut state = TradingState::new("AAPL", "2026-01-01");
    state.set_technical_indicators(TechnicalData {
        rsi: None,
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
        summary: "stale technical summary".to_owned(),
        options_summary: Some("stale options data".to_owned()),
        options_context: None,
    });

    assert!(
        state.technical_indicators().is_some(),
        "precondition: technical_indicators must be Some before clear"
    );
    assert!(
        state
            .technical_indicators()
            .unwrap()
            .options_summary
            .is_some(),
        "precondition: options_summary must be Some before clear"
    );

    state.clear_equity();

    assert!(
        state.technical_indicators().is_none(),
        "clear_equity must clear technical_indicators (and therefore options_summary)"
    );
}

#[test]
fn clear_equity_resets_options_context_unit() {
    let mut state = TradingState::new("AAPL", "2026-01-01");
    state.set_technical_indicators(TechnicalData {
        rsi: None,
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
        summary: "stale".to_owned(),
        options_summary: Some("stale interpretation".to_owned()),
        options_context: Some(crate::state::TechnicalOptionsContext::FetchFailed {
            reason: "stale provider failure".to_owned(),
        }),
    });

    state.clear_equity();
    assert!(state.technical_indicators().is_none());
}

#[tokio::test]
async fn run_analysis_cycle_clears_stale_options_summary_from_reused_state() {
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
    let (snapshot_store, _dir) =
        test_snapshot_store("pipeline-clears-stale-options-summary.db").await;
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

    // Seed an initial state with stale options_summary in technical_indicators.
    let mut initial_state = TradingState::new("AAPL", "2026-03-20");
    initial_state.set_technical_indicators(TechnicalData {
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
        summary: "stale technical summary".to_owned(),
        options_summary: Some("stale options summary from previous run".to_owned()),
        options_context: None,
    });

    let final_state = pipeline
        .run_analysis_cycle(initial_state)
        .await
        .expect("pipeline must succeed with one missing analyst input");

    // options_summary must be cleared because reset_cycle_outputs calls clear_equity()
    // before the cycle starts.
    assert!(
        final_state
            .technical_indicators()
            .map(|t| t.options_summary.is_none())
            .unwrap_or(true),
        "stale options_summary must be cleared by reset_cycle_outputs"
    );
}

fn make_pipeline(db_name: &'static str) -> (crate::workflow::TradingPipeline, tempfile::TempDir) {
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
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join(db_name);
    let store = futures::executor::block_on(SnapshotStore::new(Some(&db_path)))
        .expect("snapshot store");
    let pipeline = crate::workflow::TradingPipeline::new(
        config,
        crate::data::FinnhubClient::for_test(),
        crate::data::FredClient::for_test(),
        crate::data::YFinanceClient::new(crate::rate_limit::SharedRateLimiter::new(
            "pipeline-test",
            10,
        )),
        store,
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::providers::factory::CompletionModelHandle::for_test(),
    );
    (pipeline, dir)
}

#[tokio::test]
async fn run_analysis_cycle_preserves_options_context_in_technical_state() {
    use crate::agents::trader::build_prompt_context_for_test as build_prompt_context;
    use crate::data::traits::{OptionsOutcome, OptionsSnapshot};

    let (pipeline, _dir) = make_pipeline("pipeline-preserves-options-context.db");

    let technical_data = TechnicalData {
        rsi: Some(55.0),
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
        summary: "stub: technical with snapshot".to_owned(),
        options_summary: Some("snapshot options summary".to_owned()),
        options_context: Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(OptionsSnapshot {
                spot_price: 100.0,
                atm_iv: 0.25,
                iv_term_structure: vec![],
                put_call_volume_ratio: 0.8,
                put_call_oi_ratio: 0.9,
                max_pain_strike: 100.0,
                near_term_expiration: "2026-05-16".to_owned(),
                near_term_strikes: vec![],
            }),
        }),
    };

    replace_with_stubs_using_technical(&pipeline, Arc::clone(&pipeline.snapshot_store), technical_data)
        .expect("stub install must succeed");

    let final_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect("pipeline must succeed");

    let technical = final_state
        .technical_indicators()
        .expect("technical_indicators must be present after a successful cycle");
    assert!(
        matches!(technical.options_context, Some(TechnicalOptionsContext::Available { .. })),
        "Available options_context must survive the full pipeline cycle"
    );

    // Verify downstream prompt rendering includes the structured options_context key.
    let ctx = build_prompt_context(&final_state, "AAPL", "2026-03-20");
    assert!(
        ctx.system_prompt.contains(r#""options_context""#),
        "compact technical report must include options_context JSON key in trader prompt: {}",
        &ctx.system_prompt[..ctx.system_prompt.len().min(500)]
    );
}

#[tokio::test]
async fn run_analysis_cycle_preserves_fetch_failed_options_context_and_coherent_prompt() {
    use crate::agents::trader::build_prompt_context_for_test as build_prompt_context;

    let (pipeline, _dir) = make_pipeline("pipeline-preserves-fetch-failed-options.db");

    let technical_data = TechnicalData {
        rsi: Some(50.0),
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
        summary: "stub: technical with fetch failed".to_owned(),
        options_summary: None,
        options_context: Some(TechnicalOptionsContext::FetchFailed {
            reason: "timeout".to_owned(),
        }),
    };

    replace_with_stubs_using_technical(&pipeline, Arc::clone(&pipeline.snapshot_store), technical_data)
        .expect("stub install must succeed");

    let final_state = pipeline
        .run_analysis_cycle(TradingState::new("AAPL", "2026-03-20"))
        .await
        .expect("pipeline must succeed even when options prefetch failed");

    let technical = final_state
        .technical_indicators()
        .expect("technical_indicators must be present");
    assert!(
        matches!(
            technical.options_context,
            Some(TechnicalOptionsContext::FetchFailed { .. })
        ),
        "FetchFailed options_context must survive the pipeline cycle"
    );

    // Downstream prompt rendering must not panic and must include the
    // options_context key with fetch_failed status so agents know to ignore it.
    let ctx = build_prompt_context(&final_state, "AAPL", "2026-03-20");
    assert!(
        ctx.system_prompt.contains(r#""options_context""#),
        "FetchFailed must produce an options_context JSON key so agents see the failure status"
    );
    assert!(
        ctx.system_prompt.contains("fetch_failed"),
        "FetchFailed options_context must carry fetch_failed status in the prompt"
    );
    assert!(
        !ctx.system_prompt.contains(r#""status":"available""#),
        "FetchFailed must not claim available status in the prompt"
    );
}
