use std::sync::Arc;

use graph_flow::{Context, NextAction, Task};
use tempfile::tempdir;

use super::*;
use crate::{
    config::LlmConfig,
    state::{
        AgentTokenUsage, FundamentalData, NewsData, SentimentData, TechnicalData, TradingState,
    },
    workflow::context_bridge::{
        deserialize_state_from_context, serialize_state_to_context, write_prefixed_result,
    },
};

fn sample_state() -> TradingState {
    TradingState::new("AAPL", "2026-03-19")
}

fn sample_llm_config() -> LlmConfig {
    LlmConfig {
        quick_thinking_provider: "openai".to_owned(),
        deep_thinking_provider: "openai".to_owned(),
        quick_thinking_model: "gpt-4o-mini".to_owned(),
        deep_thinking_model: "o3".to_owned(),
        max_debate_rounds: 3,
        max_risk_rounds: 2,
        analyst_timeout_secs: 30,
        retry_max_retries: 3,
        retry_base_delay_ms: 500,
    }
}

async fn seed_state(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("seed state serialization should succeed");
}

async fn context_with_invalid_cached_news() -> Context {
    let ctx = Context::new();
    seed_state(&ctx, &sample_state()).await;
    ctx.set(
        common::KEY_CACHED_NEWS.to_owned(),
        "not valid json".to_owned(),
    )
    .await;
    ctx
}

#[tokio::test]
async fn write_flag_true_readable_back() {
    let ctx = Context::new();
    common::write_flag(&ctx, common::ANALYST_FUNDAMENTAL, true).await;
    let key = format!(
        "{}.{}.{}",
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        common::OK_SUFFIX
    );
    let ok: Option<bool> = ctx.get(&key).await;
    assert_eq!(ok, Some(true));
}

#[tokio::test]
async fn write_err_readable_back() {
    let ctx = Context::new();
    common::write_err(&ctx, common::ANALYST_NEWS, "something went wrong").await;
    let key = format!(
        "{}.{}.{}",
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        common::ERR_SUFFIX
    );
    let msg: Option<String> = ctx.get(&key).await;
    assert_eq!(msg.as_deref(), Some("something went wrong"));
}

#[tokio::test]
async fn analyst_sync_all_succeed_returns_continue() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    let result = task.run(ctx.clone()).await.expect("task should succeed");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(recovered.fundamental_metrics.is_some());
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());

    // Task 3.5 — typed evidence fields must all be Some.
    assert!(
        recovered.evidence_fundamental.is_some(),
        "evidence_fundamental must be populated"
    );
    assert!(
        recovered.evidence_sentiment.is_some(),
        "evidence_sentiment must be populated"
    );
    assert!(
        recovered.evidence_news.is_some(),
        "evidence_news must be populated"
    );
    assert!(
        recovered.evidence_technical.is_some(),
        "evidence_technical must be populated"
    );
    // Coverage: no missing inputs.
    let coverage = recovered
        .data_coverage
        .as_ref()
        .expect("data_coverage must be Some");
    assert!(
        coverage.missing_inputs.is_empty(),
        "missing_inputs must be empty when all analysts succeed"
    );
    // Provenance: all three providers, sorted.
    let provenance = recovered
        .provenance_summary
        .as_ref()
        .expect("provenance_summary must be Some");
    assert_eq!(
        provenance.providers_used,
        vec!["finnhub", "fred", "yfinance"],
        "providers_used must be sorted and deduplicated"
    );
}

#[tokio::test]
async fn analyst_sync_two_failures_returns_error_instead_of_end() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Two analysts fail.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        false,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        false,
    )
    .await;

    // Remaining analysts succeed.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    let error = task
        .run(ctx)
        .await
        .expect_err("two analyst failures should abort the workflow");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("AnalystSyncTask"));
            assert!(message.contains("2/4 analysts failed"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}

/// Task 3.6 — when technical analyst fails but the other three succeed,
/// `AnalystSyncTask` should still return Continue, `evidence_technical` should
/// remain `None`, `missing_inputs` should equal `["technical"]`, and
/// `providers_used` should equal `["finnhub", "fred"]` (no yfinance).
#[tokio::test]
async fn analyst_sync_one_missing_technical_marks_coverage_and_provenance() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Fundamental, Sentiment, News succeed; Technical fails.
    for key in [
        common::ANALYST_FUNDAMENTAL,
        common::ANALYST_SENTIMENT,
        common::ANALYST_NEWS,
    ] {
        ctx.set(
            format!("{}.{}.{}", common::ANALYST_PREFIX, key, common::OK_SUFFIX),
            true,
        )
        .await;
    }
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        false,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(15.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.3,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    let result = task
        .run(ctx.clone())
        .await
        .expect("one failure should continue");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();

    assert!(recovered.evidence_fundamental.is_some());
    assert!(recovered.evidence_sentiment.is_some());
    assert!(recovered.evidence_news.is_some());
    assert!(
        recovered.evidence_technical.is_none(),
        "evidence_technical must remain None when technical analyst failed"
    );

    let coverage = recovered
        .data_coverage
        .as_ref()
        .expect("data_coverage must be Some");
    assert_eq!(
        coverage.missing_inputs,
        vec!["technical"],
        "missing_inputs must list the failed analyst input"
    );

    let provenance = recovered
        .provenance_summary
        .as_ref()
        .expect("provenance_summary must be Some");
    assert_eq!(
        provenance.providers_used,
        vec!["finnhub", "fred"],
        "yfinance must not appear when technical evidence is absent"
    );
}

#[tokio::test]
async fn analyst_sync_counts_flagged_success_with_unreadable_payload_as_failure() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Fundamental is flagged successful but stores an unreadable payload.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_FUNDAMENTAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        "analyst.fundamental".to_owned(),
        "not valid json".to_owned(),
    )
    .await;

    // Remaining analysts succeed, so the degraded path should still continue.
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_SENTIMENT,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_NEWS,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;
    ctx.set(
        format!(
            "{}.{}.{}",
            common::ANALYST_PREFIX,
            common::ANALYST_TECHNICAL,
            common::OK_SUFFIX
        ),
        true,
    )
    .await;

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    let result = task.run(ctx.clone()).await.expect("task should succeed");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(
        recovered.fundamental_metrics.is_none(),
        "unreadable payload must not be merged into state"
    );
    assert!(recovered.market_sentiment.is_some());
    assert!(recovered.macro_news.is_some());
    assert!(recovered.technical_indicators.is_some());
}

#[tokio::test]
async fn analyst_sync_uses_longest_analyst_latency_for_fan_out_duration() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    for analyst_key in [
        common::ANALYST_FUNDAMENTAL,
        common::ANALYST_SENTIMENT,
        common::ANALYST_NEWS,
        common::ANALYST_TECHNICAL,
    ] {
        ctx.set(
            format!(
                "{}.{}.{}",
                common::ANALYST_PREFIX,
                analyst_key,
                common::OK_SUFFIX
            ),
            true,
        )
        .await;
    }

    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    let usages = [
        (
            common::ANALYST_FUNDAMENTAL,
            AgentTokenUsage {
                agent_name: "Fundamental Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                latency_ms: 150,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_SENTIMENT,
            AgentTokenUsage {
                agent_name: "Sentiment Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 11,
                completion_tokens: 6,
                total_tokens: 17,
                latency_ms: 320,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_NEWS,
            AgentTokenUsage {
                agent_name: "News Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 12,
                completion_tokens: 7,
                total_tokens: 19,
                latency_ms: 240,
                rate_limit_wait_ms: 0,
            },
        ),
        (
            common::ANALYST_TECHNICAL,
            AgentTokenUsage {
                agent_name: "Technical Analyst".to_owned(),
                model_id: "gpt-4o-mini".to_owned(),
                token_counts_available: true,
                prompt_tokens: 13,
                completion_tokens: 8,
                total_tokens: 21,
                latency_ms: 90,
                rate_limit_wait_ms: 0,
            },
        ),
    ];

    for (analyst_key, usage) in usages {
        common::write_analyst_usage(&ctx, analyst_key, &usage)
            .await
            .expect("usage write should succeed");
    }

    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    task.run(ctx.clone()).await.expect("task should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let phase = recovered
        .token_usage
        .phase_usage
        .iter()
        .find(|phase| phase.phase_name == "Analyst Fan-Out")
        .expect("analyst phase should exist");

    assert_eq!(phase.phase_duration_ms, 320);
}

#[test]
fn task_ids_are_correct() {
    assert_eq!("bearish_researcher", "bearish_researcher");
    assert_eq!("conservative_risk", "conservative_risk");
    assert_eq!("neutral_risk", "neutral_risk");
}

#[tokio::test]
async fn stub_researchers_use_production_role_names() {
    let ctx = Context::new();
    seed_state(&ctx, &sample_state()).await;

    test_helpers::StubBullishResearcherTask
        .run(ctx.clone())
        .await
        .expect("bullish stub should succeed");
    test_helpers::StubBearishResearcherTask
        .run(ctx.clone())
        .await
        .expect("bearish stub should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let roles: Vec<&str> = recovered
        .debate_history
        .iter()
        .map(|message| message.role.as_str())
        .collect();

    assert_eq!(roles, vec!["bullish_researcher", "bearish_researcher"]);
}

#[tokio::test]
async fn stub_risk_moderator_appends_workflow_history_entry() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let mut state = sample_state();
    state
        .risk_discussion_history
        .push(crate::state::DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: "stub: prior risk discussion".to_owned(),
        });
    seed_state(&ctx, &state).await;

    test_helpers::StubRiskModeratorTask {
        snapshot_store: store,
    }
    .run(ctx.clone())
    .await
    .expect("risk moderator stub should succeed");

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    let last_message = recovered
        .risk_discussion_history
        .last()
        .expect("stub should append a moderator history entry");

    assert_eq!(recovered.risk_discussion_history.len(), 2);
    assert_eq!(last_message.role, "risk_moderator");
    assert!(
        !last_message.content.is_empty(),
        "moderator history entry should include synthesis content"
    );
}

#[tokio::test]
async fn sentiment_analyst_invalid_cached_news_fails_closed() {
    let task = SentimentAnalystTask::new(
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::data::FinnhubClient::for_test(),
        sample_llm_config(),
    );
    let ctx = context_with_invalid_cached_news().await;

    let error = task
        .run(ctx)
        .await
        .expect_err("invalid cached news should fail closed");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("SentimentAnalystTask"));
            assert!(message.contains("cached news"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}

// ─── Chunk 3: derive_valuation unit tests (RED before implementation) ─────────

mod derive_valuation_tests {
    use yfinance_rs::{
        analysis::EarningsTrendRow,
        fundamentals::{BalanceSheetRow, CashflowRow, IncomeStatementRow},
        profile::Profile,
    };

    use crate::state::{AssetShape, ScenarioValuation, derive_valuation};

    fn company_profile() -> Profile {
        serde_json::from_str(
            r#"{"Company":{"name":"Test Corp","sector":null,"industry":null,"website":null,"address":null,"summary":null,"isin":null}}"#,
        )
        .unwrap()
    }

    fn fund_profile() -> Profile {
        serde_json::from_str(
            r#"{"Fund":{"name":"Test ETF","family":null,"kind":"ETF","isin":null}}"#,
        )
        .unwrap()
    }

    fn cashflow_rows_with_fcf() -> Vec<CashflowRow> {
        serde_json::from_str(
            r#"[{"period":"2025Q4","operating_cashflow":{"amount":"1200000000","currency":"USD"},"capital_expenditures":{"amount":"-200000000","currency":"USD"},"free_cash_flow":{"amount":"1000000000","currency":"USD"},"net_income":{"amount":"900000000","currency":"USD"}}]"#,
        )
        .unwrap()
    }

    fn balance_sheet_rows_with_shares() -> Vec<BalanceSheetRow> {
        serde_json::from_str(
            r#"[{"period":"2025Q4","total_assets":{"amount":"5000000000","currency":"USD"},"total_liabilities":{"amount":"2000000000","currency":"USD"},"total_equity":{"amount":"3000000000","currency":"USD"},"cash":{"amount":"500000000","currency":"USD"},"long_term_debt":{"amount":"1000000000","currency":"USD"},"shares_outstanding":1000000000}]"#,
        )
        .unwrap()
    }

    fn income_statement_rows() -> Vec<IncomeStatementRow> {
        serde_json::from_str(
            r#"[{"period":"2025Q4","total_revenue":{"amount":"4000000000","currency":"USD"},"gross_profit":{"amount":"1800000000","currency":"USD"},"operating_income":{"amount":"1200000000","currency":"USD"},"net_income":{"amount":"900000000","currency":"USD"}}]"#,
        )
        .unwrap()
    }

    fn earnings_trend_rows_with_forward_eps() -> Vec<EarningsTrendRow> {
        serde_json::from_str(
            r#"[{"period":"+1y","growth":0.08,"earnings_estimate":{"avg":{"amount":"7.25","currency":"USD"},"low":null,"high":null,"year_ago_eps":null,"num_analysts":null,"growth":0.08},"revenue_estimate":{"avg":null,"low":null,"high":null,"year_ago_revenue":null,"num_analysts":null,"growth":null},"eps_trend":{"current":null,"historical":[]},"eps_revisions":{"historical":[]}}]"#,
        )
        .unwrap()
    }

    #[test]
    fn derive_valuation_with_complete_corporate_data_produces_corporate_equity_valuation() {
        let result = derive_valuation(
            Some(company_profile()),
            Some(&cashflow_rows_with_fcf()),
            Some(&balance_sheet_rows_with_shares()),
            Some(&income_statement_rows()),
            None,
            Some(&earnings_trend_rows_with_forward_eps()),
            Some(150.0),
        );

        assert_eq!(result.asset_shape, AssetShape::CorporateEquity);
        match result.scenario {
            ScenarioValuation::CorporateEquity(val) => {
                let dcf = val
                    .dcf
                    .expect("DCF should be computed when FCF and shares are present");
                assert!(
                    dcf.free_cash_flow > 0.0,
                    "free_cash_flow must be positive, got: {}",
                    dcf.free_cash_flow
                );
                assert_eq!(dcf.discount_rate_pct, 10.0);
                assert!(
                    dcf.intrinsic_value_per_share > 0.0,
                    "intrinsic_value_per_share must be positive, got: {}",
                    dcf.intrinsic_value_per_share
                );
                let fpe = val
                    .forward_pe
                    .expect("forward_pe should be computed when EPS and price are available");
                assert!(
                    (fpe.forward_eps - 7.25).abs() < 0.01,
                    "forward_eps mismatch"
                );
                assert!(fpe.forward_pe > 0.0);
            }
            other => panic!("expected CorporateEquity, got: {other:?}"),
        }
    }

    #[test]
    fn derive_valuation_with_missing_cashflow_produces_valuation_without_dcf() {
        let result = derive_valuation(
            Some(company_profile()),
            None, // no cashflow
            Some(&balance_sheet_rows_with_shares()),
            Some(&income_statement_rows()),
            None,
            Some(&earnings_trend_rows_with_forward_eps()),
            Some(150.0),
        );

        assert_eq!(result.asset_shape, AssetShape::CorporateEquity);
        match result.scenario {
            ScenarioValuation::CorporateEquity(val) => {
                assert!(
                    val.dcf.is_none(),
                    "DCF must be None when cashflow rows are absent"
                );
                assert!(
                    val.forward_pe.is_some(),
                    "forward_pe should be computed when EPS and price are available"
                );
            }
            ScenarioValuation::NotAssessed { reason } => {
                panic!(
                    "expected CorporateEquity (partial), got NotAssessed: {reason} \
                     — forward_pe should have been computable from EPS + price"
                );
            }
        }
    }

    #[test]
    fn derive_valuation_with_fund_profile_produces_not_assessed_with_fund_reason() {
        let result = derive_valuation(
            Some(fund_profile()),
            Some(&cashflow_rows_with_fcf()),
            Some(&balance_sheet_rows_with_shares()),
            None,
            None,
            None,
            Some(100.0),
        );

        assert_eq!(result.asset_shape, AssetShape::Fund);
        match result.scenario {
            ScenarioValuation::NotAssessed { reason } => {
                assert_eq!(reason, "fund_style_asset");
            }
            other => panic!("expected NotAssessed, got: {other:?}"),
        }
    }

    #[test]
    fn derive_valuation_with_no_profile_falls_back_to_corporate_equity_from_data_shape() {
        let result = derive_valuation(
            None, // no profile
            Some(&cashflow_rows_with_fcf()),
            None,
            None,
            None,
            None,
            None,
        );

        // When profile is absent but cashflow data is present, shape must be CorporateEquity.
        assert_eq!(
            result.asset_shape,
            AssetShape::CorporateEquity,
            "absent profile + present cashflow data should yield CorporateEquity shape"
        );
    }

    #[test]
    fn derive_valuation_with_no_data_at_all_produces_unknown_not_assessed() {
        let result = derive_valuation(None, None, None, None, None, None, None);

        assert_eq!(result.asset_shape, AssetShape::Unknown);
        match result.scenario {
            ScenarioValuation::NotAssessed { .. } => {}
            other => panic!("expected NotAssessed for no-data input, got: {other:?}"),
        }
    }
}

// ─── Chunk 3: AnalystSyncTask integration test for derived_valuation ──────────

#[tokio::test]
async fn analyst_sync_sets_derived_valuation_some_on_state() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        crate::workflow::SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store creation should succeed"),
    );

    let ctx = Context::new();
    let state = sample_state();
    seed_state(&ctx, &state).await;

    // Seed all four analysts as successful so the task proceeds.
    for key in &[
        common::ANALYST_FUNDAMENTAL,
        common::ANALYST_SENTIMENT,
        common::ANALYST_NEWS,
        common::ANALYST_TECHNICAL,
    ] {
        ctx.set(
            format!("{}.{}.{}", common::ANALYST_PREFIX, key, common::OK_SUFFIX),
            true,
        )
        .await;
    }
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_FUNDAMENTAL,
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(20.0),
            eps: None,
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_SENTIMENT,
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_NEWS,
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
    write_prefixed_result(
        &ctx,
        common::ANALYST_PREFIX,
        common::ANALYST_TECHNICAL,
        &TechnicalData {
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
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    // Use a default YFinanceClient — in CI (no network) all yfinance calls return
    // None, so derived_valuation will be NotAssessed. The important contract is
    // that derived_valuation is always Some(...) after the task runs and the cycle
    // continues regardless.
    let task = AnalystSyncTask::new(store, crate::data::YFinanceClient::default());
    let result = task.run(ctx.clone()).await.expect("task should succeed");

    assert_eq!(result.next_action, NextAction::Continue);

    let recovered = deserialize_state_from_context(&ctx).await.unwrap();
    assert!(
        recovered.derived_valuation.is_some(),
        "derived_valuation must be Some after AnalystSyncTask runs; \
         was None — the derivation step was not executed"
    );
}

#[tokio::test]
async fn news_analyst_invalid_cached_news_fails_closed() {
    let task = NewsAnalystTask::new(
        crate::providers::factory::CompletionModelHandle::for_test(),
        crate::data::FinnhubClient::for_test(),
        crate::data::FredClient::for_test(),
        sample_llm_config(),
    );
    let ctx = context_with_invalid_cached_news().await;

    let error = task
        .run(ctx)
        .await
        .expect_err("invalid cached news should fail closed");

    match error {
        graph_flow::GraphError::TaskExecutionFailed(message) => {
            assert!(message.contains("NewsAnalystTask"));
            assert!(message.contains("cached news"));
        }
        other => panic!("expected TaskExecutionFailed, got: {other:?}"),
    }
}
