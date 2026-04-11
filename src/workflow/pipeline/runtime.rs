use std::sync::Arc;

use graph_flow::{
    ExecutionStatus, FlowRunner, Graph, InMemorySessionStorage, Session, SessionStorage,
    fanout::FanOutTask,
};
use tracing::{error, info, instrument};
use uuid::Uuid;

use crate::{
    analysis_packs::resolve_runtime_policy,
    config::Config,
    data::adapters::{
        EnrichmentResult, EnrichmentStatus,
        estimates::{ConsensusEvidence, EstimatesProvider, YFinanceEstimatesProvider},
        events::{EventNewsEvidence, EventNewsProvider, FinnhubEventNewsProvider},
    },
    data::{FinnhubClient, FredClient, YFinanceClient},
    error::TradingError,
    providers::factory::CompletionModelHandle,
    state::{EnrichmentState, TradingState},
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        tasks::{
            AggressiveRiskTask, AnalystSyncTask, BearishResearcherTask, BullishResearcherTask,
            ConservativeRiskTask, DebateModeratorTask, FundManagerTask, FundamentalAnalystTask,
            KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_NEWS, KEY_DEBATE_ROUND,
            KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND, NeutralRiskTask,
            NewsAnalystTask, PreflightTask, RiskModeratorTask, SentimentAnalystTask,
            TechnicalAnalystTask, TraderTask,
        },
    },
};

use super::{MAX_PIPELINE_STEPS, TradingPipeline, constants::TASKS, errors};

pub(super) fn canonicalize_runtime_symbol(symbol: &str) -> Result<String, TradingError> {
    Ok(crate::data::resolve_symbol(symbol)?.canonical_symbol)
}

pub(super) fn reset_cycle_outputs(state: &mut TradingState) {
    state.current_price = None;
    state.market_volatility = None;
    state.fundamental_metrics = None;
    state.technical_indicators = None;
    state.market_sentiment = None;
    state.macro_news = None;
    state.evidence_fundamental = None;
    state.evidence_technical = None;
    state.evidence_sentiment = None;
    state.evidence_news = None;
    state.enrichment_event_news = EnrichmentState::default();
    state.enrichment_consensus = EnrichmentState::default();
    state.data_coverage = None;
    state.provenance_summary = None;
    state.debate_history.clear();
    state.consensus_summary = None;
    state.trader_proposal = None;
    state.risk_discussion_history.clear();
    state.aggressive_risk_report = None;
    state.neutral_risk_report = None;
    state.conservative_risk_report = None;
    state.final_execution_status = None;
    // Thesis memory is reset here so stale thesis never leaks across reused
    // runs. Preflight will reload `prior_thesis` from the snapshot store for
    // the current canonical symbol; FundManagerTask will set `current_thesis`.
    state.prior_thesis = None;
    state.current_thesis = None;
    state.derived_valuation = None;
    state.analysis_pack_name = None;
    state.analysis_runtime_policy = None;
    state.token_usage = Default::default();
}

pub(super) fn build_graph(
    config: Arc<Config>,
    finnhub: &FinnhubClient,
    fred: &FredClient,
    yfinance: &YFinanceClient,
    snapshot_store: Arc<SnapshotStore>,
    quick_handle: &CompletionModelHandle,
    deep_handle: &CompletionModelHandle,
) -> Arc<Graph> {
    let graph = Arc::new(Graph::new("trading_pipeline"));

    let preflight = PreflightTask::with_pack(
        config.enrichment.clone(),
        Arc::clone(&snapshot_store),
        config.analysis_pack.clone(),
    );
    graph.add_task(Arc::new(preflight));

    let fan_out = FanOutTask::new(
        TASKS.analyst_fan_out,
        vec![
            FundamentalAnalystTask::new(quick_handle.clone(), finnhub.clone(), config.llm.clone()),
            SentimentAnalystTask::new(quick_handle.clone(), finnhub.clone(), config.llm.clone()),
            NewsAnalystTask::new(
                quick_handle.clone(),
                finnhub.clone(),
                fred.clone(),
                config.llm.clone(),
            ),
            TechnicalAnalystTask::new(quick_handle.clone(), yfinance.clone(), config.llm.clone()),
        ],
    );
    graph.add_task(fan_out);
    graph.add_edge(TASKS.preflight, TASKS.analyst_fan_out);

    let analyst_sync = AnalystSyncTask::with_yfinance(
        Arc::clone(&snapshot_store),
        yfinance.clone(),
        std::time::Duration::from_secs(config.llm.valuation_fetch_timeout_secs),
    );
    graph.add_task(analyst_sync);
    graph.add_edge(TASKS.analyst_fan_out, TASKS.analyst_sync);

    graph.add_conditional_edge(
        TASKS.analyst_sync,
        |ctx| ctx.get_sync::<u32>(KEY_MAX_DEBATE_ROUNDS).unwrap_or(0) > 0,
        TASKS.bullish_researcher,
        TASKS.debate_moderator,
    );

    graph.add_task(BullishResearcherTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
    ));
    graph.add_task(BearishResearcherTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
    ));
    graph.add_task(DebateModeratorTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
        Arc::clone(&snapshot_store),
    ));

    graph.add_edge(TASKS.bullish_researcher, TASKS.bearish_researcher);
    graph.add_edge(TASKS.bearish_researcher, TASKS.debate_moderator);
    graph.add_conditional_edge(
        TASKS.debate_moderator,
        |ctx| {
            let round = ctx.get_sync::<u32>(KEY_DEBATE_ROUND).unwrap_or(0);
            let max = ctx.get_sync::<u32>(KEY_MAX_DEBATE_ROUNDS).unwrap_or(0);
            round < max
        },
        TASKS.bullish_researcher,
        TASKS.trader,
    );

    graph.add_task(TraderTask::new(
        Arc::clone(&config),
        Arc::clone(&snapshot_store),
    ));
    graph.add_conditional_edge(
        TASKS.trader,
        |ctx| ctx.get_sync::<u32>(KEY_MAX_RISK_ROUNDS).unwrap_or(0) > 0,
        TASKS.aggressive_risk,
        TASKS.risk_moderator,
    );

    graph.add_task(AggressiveRiskTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
    ));
    graph.add_task(ConservativeRiskTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
    ));
    graph.add_task(NeutralRiskTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
    ));
    graph.add_task(RiskModeratorTask::new(
        Arc::clone(&config),
        deep_handle.clone(),
        Arc::clone(&snapshot_store),
    ));

    graph.add_edge(TASKS.aggressive_risk, TASKS.conservative_risk);
    graph.add_edge(TASKS.conservative_risk, TASKS.neutral_risk);
    graph.add_edge(TASKS.neutral_risk, TASKS.risk_moderator);
    graph.add_conditional_edge(
        TASKS.risk_moderator,
        |ctx| {
            let round = ctx.get_sync::<u32>(KEY_RISK_ROUND).unwrap_or(0);
            let max = ctx.get_sync::<u32>(KEY_MAX_RISK_ROUNDS).unwrap_or(0);
            round < max
        },
        TASKS.aggressive_risk,
        TASKS.fund_manager,
    );

    graph.add_task(FundManagerTask::new(
        Arc::clone(&config),
        Arc::clone(&snapshot_store),
    ));
    graph.set_start_task(TASKS.preflight);
    graph
}

#[instrument(skip(pipeline, initial_state), fields(symbol = %initial_state.asset_symbol, date = %initial_state.target_date))]
pub(super) async fn run_analysis_cycle(
    pipeline: &TradingPipeline,
    mut initial_state: TradingState,
) -> Result<TradingState, TradingError> {
    reset_cycle_outputs(&mut initial_state);
    initial_state.execution_id = Uuid::new_v4();
    initial_state.asset_symbol = canonicalize_runtime_symbol(&initial_state.asset_symbol)?;

    let runtime_policy =
        resolve_runtime_policy(&pipeline.config.analysis_pack).map_err(|cause| {
            TradingError::Config(anyhow::anyhow!(
                "analysis pack resolution failed for '{}': {cause}",
                pipeline.config.analysis_pack
            ))
        })?;

    // Persist pack metadata and the resolved runtime policy on state so all
    // downstream consumers can read the same typed policy surface.
    initial_state.analysis_pack_name = Some(pipeline.config.analysis_pack.clone());
    initial_state.analysis_runtime_policy = Some(runtime_policy.clone());

    let symbol = initial_state.asset_symbol.clone();
    let date = initial_state.target_date.clone();
    let execution_id = initial_state.execution_id.to_string();
    info!(symbol = %symbol, date = %date, execution_id = %execution_id, "cycle started");

    let need_price = initial_state.current_price.is_none();
    let need_vix = initial_state.market_volatility.is_none();
    let (price_result, vix_result, news_result) = {
        use crate::agents::analyst::prefetch_analyst_news;
        tokio::join!(
            async {
                if need_price {
                    crate::data::get_latest_close(&pipeline.yfinance, &symbol, &date).await
                } else {
                    None
                }
            },
            async {
                if need_vix {
                    crate::data::fetch_vix_data(&pipeline.yfinance, &date).await
                } else {
                    None
                }
            },
            prefetch_analyst_news(&pipeline.finnhub, &symbol),
        )
    };

    if let Some(price) = price_result {
        info!(symbol = %symbol, price, "fetched current price from yfinance");
        initial_state.current_price = Some(price);
    } else if need_price {
        info!(symbol = %symbol, "current price unavailable from yfinance");
    }

    if let Some(vix) = vix_result {
        info!(
            vix_level = vix.vix_level,
            regime = %vix.vix_regime,
            trend = %vix.vix_trend,
            "fetched VIX market volatility context"
        );
        initial_state.market_volatility = Some(vix);
    } else if need_vix {
        info!("VIX data unavailable; continuing without volatility context");
    }

    // ── Enrichment hydration (fail-open, timeout-bounded) ─────────────
    // Results are stored both on TradingState (for downstream agent/report
    // consumers) and in the context cache keys (for preflight compatibility).
    let enrichment_cfg = &pipeline.config.enrichment;
    let enrichment_intent = &runtime_policy.enrichment_intent;
    let fetch_timeout = std::time::Duration::from_secs(enrichment_cfg.fetch_timeout_secs);

    let event_enrichment = if enrichment_intent.event_news {
        hydrate_event_news(&pipeline.finnhub, &symbol, &date, fetch_timeout).await
    } else {
        EnrichmentResult::NotAvailable
    };

    let consensus_enrichment = if enrichment_intent.consensus_estimates {
        hydrate_consensus(&pipeline.yfinance, &symbol, &date, fetch_timeout).await
    } else {
        EnrichmentResult::NotAvailable
    };

    // Persist enrichment results on state for downstream consumers.
    initial_state.enrichment_event_news = EnrichmentState {
        status: if enrichment_intent.event_news {
            event_enrichment.status()
        } else {
            EnrichmentStatus::Disabled
        },
        payload: event_enrichment.into_option(),
    };
    initial_state.enrichment_consensus = EnrichmentState {
        status: if enrichment_intent.consensus_estimates {
            consensus_enrichment.status()
        } else {
            EnrichmentStatus::Disabled
        },
        payload: consensus_enrichment.into_option(),
    };

    let cached_news_json = news_result.and_then(|arc| serde_json::to_string(arc.as_ref()).ok());
    let graph = Arc::clone(&pipeline.graph);
    let storage = Arc::new(InMemorySessionStorage::new());
    let session_id = Uuid::new_v4().to_string();
    let session = Session::new_from_task(session_id.clone(), TASKS.preflight);

    serialize_state_to_context(&initial_state, &session.context)
        .await
        .map_err(|e| TradingError::GraphFlow {
            phase: "init".into(),
            task: "serialize_state".into(),
            cause: e.to_string(),
        })?;

    session
        .context
        .set(KEY_MAX_DEBATE_ROUNDS, pipeline.config.llm.max_debate_rounds)
        .await;
    session
        .context
        .set(KEY_MAX_RISK_ROUNDS, pipeline.config.llm.max_risk_rounds)
        .await;
    session.context.set(KEY_DEBATE_ROUND, 0u32).await;
    session.context.set(KEY_RISK_ROUND, 0u32).await;

    if let Some(news_json) = cached_news_json {
        session.context.set(KEY_CACHED_NEWS, news_json).await;
    }

    // ── Write enrichment payloads to context cache keys ──────────────
    // These are written before save; PreflightTask's `seed_if_absent` will
    // preserve non-null values rather than overwriting them.
    if let Some(ref events) = initial_state.enrichment_event_news.payload
        && let Ok(json) = serde_json::to_string(events)
    {
        info!(count = events.len(), "hydrated event-news enrichment");
        session.context.set(KEY_CACHED_EVENT_FEED, json).await;
    }
    if let Some(ref consensus) = initial_state.enrichment_consensus.payload
        && let Ok(json) = serde_json::to_string(consensus)
    {
        info!(symbol = %consensus.symbol, "hydrated consensus-estimates enrichment");
        session.context.set(KEY_CACHED_CONSENSUS, json).await;
    }

    storage
        .save(session)
        .await
        .map_err(|e| TradingError::GraphFlow {
            phase: "init".into(),
            task: "save_session".into(),
            cause: e.to_string(),
        })?;

    let runner = FlowRunner::new(graph, storage.clone());
    let result = run_pipeline_loop(&runner, &session_id).await;
    if let Err(error) = &result {
        error!(symbol = %symbol, date = %date, execution_id = %execution_id, error = %error, "cycle failed");
    }
    result?;

    let final_session = storage
        .get(&session_id)
        .await
        .map_err(|e| TradingError::GraphFlow {
            phase: "finalize".into(),
            task: "load_session".into(),
            cause: e.to_string(),
        })?
        .ok_or_else(|| TradingError::GraphFlow {
            phase: "finalize".into(),
            task: "load_session".into(),
            cause: format!("session '{session_id}' not found after completion"),
        })?;

    let final_state = deserialize_state_from_context(&final_session.context)
        .await
        .map_err(|e| TradingError::GraphFlow {
            phase: "finalize".into(),
            task: "deserialize_state".into(),
            cause: e.to_string(),
        })?;

    info!(symbol = %symbol, date = %date, execution_id = %execution_id, "cycle complete");
    Ok(final_state)
}

// ─── Enrichment hydration helpers ────────────────────────────────────────────

/// Fetch event-news enrichment with a timeout boundary.
async fn hydrate_event_news(
    finnhub: &FinnhubClient,
    symbol: &str,
    target_date: &str,
    timeout: std::time::Duration,
) -> EnrichmentResult<Vec<EventNewsEvidence>> {
    let provider = FinnhubEventNewsProvider::new(finnhub.clone());
    match tokio::time::timeout(timeout, provider.fetch_event_news(symbol, target_date)).await {
        Ok(Ok(events)) if events.is_empty() => {
            info!(symbol, "event-news enrichment: no events found");
            EnrichmentResult::NotAvailable
        }
        Ok(Ok(events)) => {
            info!(
                symbol,
                count = events.len(),
                "event-news enrichment: available"
            );
            EnrichmentResult::Available(events)
        }
        Ok(Err(e)) => {
            info!(symbol, error = %e, "event-news enrichment: fetch failed (fail-open)");
            EnrichmentResult::FetchFailed(e.to_string())
        }
        Err(_) => {
            info!(symbol, "event-news enrichment: timed out (fail-open)");
            EnrichmentResult::FetchFailed("enrichment fetch timed out".to_owned())
        }
    }
}

/// Fetch consensus-estimates enrichment with a timeout boundary.
async fn hydrate_consensus(
    yfinance: &YFinanceClient,
    symbol: &str,
    target_date: &str,
    timeout: std::time::Duration,
) -> EnrichmentResult<ConsensusEvidence> {
    if target_date
        != chrono::Utc::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    {
        info!(
            symbol,
            target_date, "consensus-estimates enrichment skipped for historical target date"
        );
        return EnrichmentResult::NotAvailable;
    }

    let provider = YFinanceEstimatesProvider::new(yfinance.clone());
    match tokio::time::timeout(timeout, provider.fetch_consensus(symbol, target_date)).await {
        Ok(Ok(Some(evidence))) => {
            info!(symbol, "consensus-estimates enrichment: available");
            EnrichmentResult::Available(evidence)
        }
        Ok(Ok(None)) => {
            info!(symbol, "consensus-estimates enrichment: no data found");
            EnrichmentResult::NotAvailable
        }
        Ok(Err(e)) => {
            info!(symbol, error = %e, "consensus-estimates enrichment: fetch failed (fail-open)");
            EnrichmentResult::FetchFailed(e.to_string())
        }
        Err(_) => {
            info!(
                symbol,
                "consensus-estimates enrichment: timed out (fail-open)"
            );
            EnrichmentResult::FetchFailed("enrichment fetch timed out".to_owned())
        }
    }
}

async fn run_pipeline_loop(runner: &FlowRunner, session_id: &str) -> Result<(), TradingError> {
    let mut step = 0usize;
    loop {
        step += 1;
        if step > MAX_PIPELINE_STEPS {
            return Err(TradingError::GraphFlow {
                phase: "pipeline_execution".into(),
                task: "step_ceiling".into(),
                cause: format!(
                    "pipeline exceeded maximum of {MAX_PIPELINE_STEPS} steps - possible runaway loop from corrupted round counters or misconfigured conditional edges"
                ),
            });
        }

        info!(step, "pipeline step");
        let result = runner
            .run(session_id)
            .await
            .map_err(errors::map_graph_error)?;

        match result.status {
            ExecutionStatus::Completed => {
                info!(steps = step, "pipeline completed");
                break;
            }
            ExecutionStatus::Paused {
                ref next_task_id, ..
            } => {
                info!(next = %next_task_id, step, "pipeline paused, continuing");
            }
            ExecutionStatus::WaitingForInput => {
                return Err(TradingError::GraphFlow {
                    phase: "pipeline_execution".into(),
                    task: "unexpected_input_wait".into(),
                    cause: "pipeline unexpectedly waiting for input".into(),
                });
            }
            ExecutionStatus::Error(ref msg) => {
                error!(error = %msg, step, "pipeline step returned error status");
                let (task_id, cause) = errors::extract_task_identity(msg);
                return Err(TradingError::GraphFlow {
                    phase: errors::phase_for_task(&task_id),
                    task: task_id,
                    cause: crate::providers::factory::sanitize_error_summary(&cause),
                });
            }
        }
    }
    Ok(())
}
