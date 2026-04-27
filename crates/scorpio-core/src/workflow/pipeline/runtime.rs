use std::sync::Arc;

use graph_flow::{
    ExecutionStatus, FlowRunner, Graph, InMemorySessionStorage, Session, SessionStorage,
};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::{
    agents::analyst::{AnalystId, AnalystRegistry},
    analysis_packs::{PackId, resolve_pack, resolve_runtime_policy},
    config::Config,
    data::adapters::{
        EnrichmentResult, EnrichmentStatus,
        estimates::{
            ConsensusEvidence, ConsensusOutcome, EstimatesProvider, YFinanceEstimatesProvider,
        },
        events::{EventNewsEvidence, EventNewsProvider, FinnhubEventNewsProvider},
    },
    data::{FinnhubClient, FredClient, YFinanceClient},
    domain::Symbol,
    error::TradingError,
    providers::factory::CompletionModelHandle,
    state::{EnrichmentState, TradingState},
    workflow::{
        SnapshotStore,
        context_bridge::{deserialize_state_from_context, serialize_state_to_context},
        tasks::{
            FundamentalAnalystTask, KEY_CACHED_CONSENSUS, KEY_CACHED_EVENT_FEED, KEY_CACHED_NEWS,
            KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND,
            NewsAnalystTask, SentimentAnalystTask, TechnicalAnalystTask,
        },
    },
};

/// After this many consecutive `ProviderDegraded` outcomes, downgrade the
/// runtime status to `NotAvailable` so a stuck consensus provider does not
/// indefinitely register as `FetchFailed`.
pub(super) const CONSENSUS_PROVIDER_DEGRADED_HALF_LIFE_CYCLES: u32 = 3;

use super::{MAX_PIPELINE_STEPS, TradingPipeline, constants::TASKS, errors};

pub(super) fn canonicalize_runtime_symbol(symbol: &str) -> Result<Symbol, TradingError> {
    Ok(crate::data::resolve_symbol(symbol)?.symbol)
}

/// Construct the ordered list of analyst fan-out tasks for `required_inputs`.
///
/// For each entry of `required_inputs` that resolves to an [`AnalystId`]
/// registered in `registry`, the matching concrete `Task` is built and pushed
/// in input order. Unknown inputs and analysts that are not registered are
/// silently dropped — consistent with the graceful-degradation contract
/// already enforced in `AnalystSyncTask::input_missing`.
///
/// For the baseline pack's input list (`["fundamentals", "sentiment",
/// "news", "technical"]`) this reproduces the previous hard-coded
/// four-analyst vector byte-for-byte.
pub(crate) fn build_analyst_tasks(
    registry: &AnalystRegistry,
    required_inputs: &[String],
    finnhub: &FinnhubClient,
    fred: &FredClient,
    yfinance: &YFinanceClient,
    quick_handle: &CompletionModelHandle,
    llm_config: &crate::config::LlmConfig,
) -> Vec<Arc<dyn graph_flow::Task>> {
    registry
        .for_inputs(required_inputs.iter().map(String::as_str))
        .into_iter()
        .filter_map(|id| build_analyst_task(id, finnhub, fred, yfinance, quick_handle, llm_config))
        .collect()
}

fn build_analyst_task(
    id: AnalystId,
    finnhub: &FinnhubClient,
    fred: &FredClient,
    yfinance: &YFinanceClient,
    quick_handle: &CompletionModelHandle,
    llm_config: &crate::config::LlmConfig,
) -> Option<Arc<dyn graph_flow::Task>> {
    // Each arm clones the provider / handle because Task construction takes
    // ownership. The crypto variants are registered (for identity) but not
    // spawnable until the crypto pack implementation lands.
    match id {
        AnalystId::Fundamental => Some(FundamentalAnalystTask::new(
            quick_handle.clone(),
            finnhub.clone(),
            llm_config.clone(),
        )),
        AnalystId::Sentiment => Some(SentimentAnalystTask::new(
            quick_handle.clone(),
            finnhub.clone(),
            llm_config.clone(),
        )),
        AnalystId::News => Some(NewsAnalystTask::new(
            quick_handle.clone(),
            finnhub.clone(),
            fred.clone(),
            llm_config.clone(),
        )),
        AnalystId::Technical => Some(TechnicalAnalystTask::new(
            quick_handle.clone(),
            yfinance.clone(),
            llm_config.clone(),
        )),
        AnalystId::Tokenomics | AnalystId::OnChain | AnalystId::Social | AnalystId::Derivatives => {
            None
        }
    }
}

pub(super) fn reset_cycle_outputs(state: &mut TradingState) {
    state.current_price = None;
    // Drop the whole equity sub-state in one move — on the next cycle
    // writers repopulate it lazily through the accessor setters.
    state.clear_equity();
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
    state.analysis_pack_name = None;
    // PreflightTask is the sole writer of `analysis_runtime_policy` per the
    // Unit 4a structural authority migration. Clearing it here is hygiene
    // for reused `TradingState` instances — preflight will write the
    // resolved policy back during its run. No call site outside preflight
    // (and the gated `testing::runtime_policy` helpers) sets this field.
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
    // Phase 7 synthesis: delegate to the pack-driven builder after
    // resolving the active pack id. If the config selects an unknown pack
    // or a non-selectable stub, fall back to the baseline manifest so the
    // graph still builds for misconfigured runs — `run_analysis_cycle`
    // re-resolves and surfaces a proper error downstream.
    let pack_id: PackId = config.analysis_pack.parse().unwrap_or(PackId::Baseline);
    let pack = resolve_pack(pack_id);
    let registry = AnalystRegistry::all_known();
    crate::workflow::builder::build_graph_from_pack(
        &pack,
        config,
        &registry,
        finnhub,
        fred,
        yfinance,
        snapshot_store,
        quick_handle,
        deep_handle,
    )
}

#[instrument(skip(pipeline, initial_state), fields(symbol = %initial_state.asset_symbol, date = %initial_state.target_date))]
pub(super) async fn run_analysis_cycle(
    pipeline: &TradingPipeline,
    mut initial_state: TradingState,
) -> Result<TradingState, TradingError> {
    // Capture the prior cycle's consensus enrichment payload BEFORE reset so
    // the half-life counter survives reused-state runs. For fresh runs the
    // payload is `None`, so the counter starts at 0 and reset has no effect.
    let prior_consensus_payload = initial_state.enrichment_consensus.payload.clone();
    reset_cycle_outputs(&mut initial_state);
    initial_state.execution_id = Uuid::new_v4();
    let canonical = canonicalize_runtime_symbol(&initial_state.asset_symbol)?;
    initial_state.set_symbol(canonical);

    let runtime_policy = match pipeline.runtime_policy.clone() {
        Some(policy) => policy,
        None => resolve_runtime_policy(&pipeline.config.analysis_pack).map_err(|cause| {
            TradingError::Config(anyhow::anyhow!(
                "analysis pack resolution failed for '{}': {cause}",
                pipeline.config.analysis_pack
            ))
        })?,
    };

    let symbol = initial_state.asset_symbol.clone();
    let date = initial_state.target_date.clone();
    let execution_id = initial_state.execution_id.to_string();
    info!(symbol = %symbol, date = %date, execution_id = %execution_id, "cycle started");

    let need_price = initial_state.current_price.is_none();
    let need_vix = initial_state.market_volatility().is_none();
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
        initial_state.set_market_volatility(vix);
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

    let consensus_enrichment_state = if enrichment_intent.consensus_estimates {
        hydrate_consensus(
            &pipeline.yfinance,
            &symbol,
            &date,
            fetch_timeout,
            prior_consensus_payload.as_ref(),
        )
        .await
    } else {
        EnrichmentState {
            status: EnrichmentStatus::Disabled,
            payload: None,
        }
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
    initial_state.enrichment_consensus = consensus_enrichment_state;

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

/// Single-shot fetch outcome surfaced to the half-life policy. Wraps the
/// upstream `Result<ConsensusOutcome, TradingError>` plus a `Timeout` variant
/// the policy converts to `FetchFailed("timeout")`.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum HydratedConsensusFetch {
    Success(ConsensusOutcome),
    Failed(String),
    TimedOut,
}

/// Fetch consensus-estimates enrichment with a timeout boundary, applying
/// the half-life + retry policy described in the implementation plan
/// (see `docs/superpowers/plans/2026-04-26-yfinance-news-options-consensus-implementation.md`).
async fn hydrate_consensus(
    yfinance: &YFinanceClient,
    symbol: &str,
    target_date: &str,
    timeout: std::time::Duration,
    prior_payload: Option<&ConsensusEvidence>,
) -> EnrichmentState<ConsensusEvidence> {
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
        return EnrichmentState {
            status: EnrichmentStatus::NotAvailable,
            payload: None,
        };
    }

    let provider = YFinanceEstimatesProvider::new(yfinance.clone());
    let primary = single_consensus_fetch(&provider, symbol, target_date, timeout).await;

    // ProviderDegraded retry: if the first attempt classifies as degraded,
    // retry once immediately (no backoff — the rate limiter handles spacing).
    let resolved = match &primary {
        HydratedConsensusFetch::Success(ConsensusOutcome::ProviderDegraded) => {
            info!(
                symbol,
                "consensus-estimates: provider_degraded; retrying once"
            );
            single_consensus_fetch(&provider, symbol, target_date, timeout).await
        }
        _ => primary,
    };

    apply_consensus_half_life_policy(symbol, &resolved, prior_payload)
}

async fn single_consensus_fetch(
    provider: &YFinanceEstimatesProvider,
    symbol: &str,
    target_date: &str,
    timeout: std::time::Duration,
) -> HydratedConsensusFetch {
    match tokio::time::timeout(timeout, provider.fetch_consensus(symbol, target_date)).await {
        Ok(Ok(outcome)) => HydratedConsensusFetch::Success(outcome),
        Ok(Err(e)) => HydratedConsensusFetch::Failed(e.to_string()),
        Err(_) => HydratedConsensusFetch::TimedOut,
    }
}

/// Pure half-life policy: maps the resolved fetch outcome plus the prior
/// cycle's persisted payload into the `EnrichmentState` we want to commit
/// for this cycle.
///
/// Behavior:
/// - `Success(Data(_))` / `Success(NoCoverage)` reset `consecutive_provider_degraded_cycles` to 0.
/// - `Success(ProviderDegraded)` increments the counter; if it reaches
///   [`CONSENSUS_PROVIDER_DEGRADED_HALF_LIFE_CYCLES`], the runtime
///   downgrades the status from `FetchFailed("provider_degraded")` to
///   `NotAvailable` and emits a warn so a stuck provider does not pin the
///   symbol at FetchFailed forever.
/// - `Failed(_)` and `TimedOut` leave the counter untouched (they are operationally
///   distinct from provider-degraded; the latter is a structured upstream
///   signal). The status reflects the failure reason.
pub(super) fn apply_consensus_half_life_policy(
    symbol: &str,
    fetch: &HydratedConsensusFetch,
    prior_payload: Option<&ConsensusEvidence>,
) -> EnrichmentState<ConsensusEvidence> {
    let prior_counter = prior_payload
        .map(|p| p.consecutive_provider_degraded_cycles)
        .unwrap_or(0);

    match fetch {
        HydratedConsensusFetch::Success(ConsensusOutcome::Data(evidence)) => {
            info!(symbol, "consensus-estimates enrichment: available");
            let mut evidence = evidence.clone();
            evidence.consecutive_provider_degraded_cycles = 0;
            EnrichmentState {
                status: EnrichmentStatus::Available,
                payload: Some(evidence),
            }
        }
        HydratedConsensusFetch::Success(ConsensusOutcome::NoCoverage) => {
            info!(
                symbol,
                "consensus-estimates enrichment: no analyst coverage"
            );
            EnrichmentState {
                status: EnrichmentStatus::NotAvailable,
                payload: None,
            }
        }
        HydratedConsensusFetch::Success(ConsensusOutcome::ProviderDegraded) => {
            let cycles = prior_counter.saturating_add(1);
            // Persist a counter-only stub so the next cycle can read the
            // running tally. The stub carries identity fields but no data —
            // all sub-fields are `None` per the ProviderDegraded contract.
            let stub = ConsensusEvidence {
                symbol: symbol.to_ascii_uppercase(),
                eps_estimate: None,
                revenue_estimate_m: None,
                analyst_count: None,
                as_of_date: chrono::Utc::now()
                    .date_naive()
                    .format("%Y-%m-%d")
                    .to_string(),
                price_target: None,
                recommendations: None,
                consecutive_provider_degraded_cycles: cycles,
            };

            if cycles >= CONSENSUS_PROVIDER_DEGRADED_HALF_LIFE_CYCLES {
                warn!(
                    symbol,
                    cycles, "provider_degraded persisted; treating as no_coverage after half-life"
                );
                EnrichmentState {
                    status: EnrichmentStatus::NotAvailable,
                    payload: Some(stub),
                }
            } else {
                info!(
                    symbol,
                    cycles, "consensus-estimates enrichment: provider degraded (fail-open)"
                );
                EnrichmentState {
                    status: EnrichmentStatus::FetchFailed("provider_degraded".to_owned()),
                    payload: Some(stub),
                }
            }
        }
        HydratedConsensusFetch::Failed(reason) => {
            info!(symbol, error = %reason, "consensus-estimates enrichment: fetch failed (fail-open)");
            EnrichmentState {
                status: EnrichmentStatus::FetchFailed(reason.clone()),
                payload: None,
            }
        }
        HydratedConsensusFetch::TimedOut => {
            info!(
                symbol,
                "consensus-estimates enrichment: timed out (fail-open)"
            );
            EnrichmentState {
                status: EnrichmentStatus::FetchFailed("timeout".to_owned()),
                payload: None,
            }
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

#[cfg(test)]
mod consensus_half_life_tests {
    use super::*;

    fn data_evidence(symbol: &str, counter: u32) -> ConsensusEvidence {
        ConsensusEvidence {
            symbol: symbol.to_owned(),
            eps_estimate: Some(2.5),
            revenue_estimate_m: Some(100.0),
            analyst_count: Some(10),
            as_of_date: "2026-04-27".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: counter,
        }
    }

    #[test]
    fn provider_degraded_downgrades_to_not_available_after_half_life() {
        // Walk through 4 hydration cycles. Each call to the policy gets the
        // prior cycle's persisted payload as input.
        let symbol = "AAPL";
        let degraded = HydratedConsensusFetch::Success(ConsensusOutcome::ProviderDegraded);

        // Cycle 1: prior counter 0 -> persists 1 with FetchFailed.
        let cycle1 = apply_consensus_half_life_policy(symbol, &degraded, None);
        assert!(
            matches!(cycle1.status, EnrichmentStatus::FetchFailed(ref reason) if reason == "provider_degraded"),
            "cycle 1 should be FetchFailed(provider_degraded), got {:?}",
            cycle1.status
        );
        let stub1 = cycle1.payload.expect("cycle 1 must persist a counter stub");
        assert_eq!(stub1.consecutive_provider_degraded_cycles, 1);
        assert!(stub1.eps_estimate.is_none());

        // Cycle 2: prior counter 1 -> persists 2 with FetchFailed.
        let cycle2 = apply_consensus_half_life_policy(symbol, &degraded, Some(&stub1));
        assert!(
            matches!(cycle2.status, EnrichmentStatus::FetchFailed(ref reason) if reason == "provider_degraded"),
            "cycle 2 should be FetchFailed(provider_degraded), got {:?}",
            cycle2.status
        );
        let stub2 = cycle2.payload.expect("cycle 2 must persist a counter stub");
        assert_eq!(stub2.consecutive_provider_degraded_cycles, 2);

        // Cycle 3: prior counter 2 -> persists 3, downgrades to NotAvailable.
        let cycle3 = apply_consensus_half_life_policy(symbol, &degraded, Some(&stub2));
        assert!(
            matches!(cycle3.status, EnrichmentStatus::NotAvailable),
            "cycle 3 should hit half-life and downgrade to NotAvailable, got {:?}",
            cycle3.status
        );
        let stub3 = cycle3.payload.expect("cycle 3 must persist a counter stub");
        assert_eq!(stub3.consecutive_provider_degraded_cycles, 3);

        // Cycle 4: prior counter 3 -> still degraded -> persists 4, still NotAvailable.
        let cycle4 = apply_consensus_half_life_policy(symbol, &degraded, Some(&stub3));
        assert!(
            matches!(cycle4.status, EnrichmentStatus::NotAvailable),
            "cycle 4 should remain NotAvailable, got {:?}",
            cycle4.status
        );
        let stub4 = cycle4.payload.expect("cycle 4 must persist a counter stub");
        assert_eq!(stub4.consecutive_provider_degraded_cycles, 4);
    }

    #[test]
    fn no_coverage_resets_counter_to_zero() {
        let symbol = "AAPL";
        // Pre-condition: a prior payload with a non-zero counter.
        let prior = ConsensusEvidence {
            consecutive_provider_degraded_cycles: 5,
            ..data_evidence(symbol, 5)
        };

        let no_coverage = HydratedConsensusFetch::Success(ConsensusOutcome::NoCoverage);
        let next = apply_consensus_half_life_policy(symbol, &no_coverage, Some(&prior));
        assert!(matches!(next.status, EnrichmentStatus::NotAvailable));
        // NoCoverage drops payload entirely; the counter is implicitly reset
        // because subsequent cycles see `None` (counter = 0).
        assert!(
            next.payload.is_none(),
            "NoCoverage should not carry a payload"
        );
    }

    #[test]
    fn data_outcome_resets_counter_to_zero() {
        let symbol = "AAPL";
        // Prior cycle was degraded with counter 2.
        let prior_stub = ConsensusEvidence {
            symbol: symbol.to_owned(),
            eps_estimate: None,
            revenue_estimate_m: None,
            analyst_count: None,
            as_of_date: "2026-04-26".to_owned(),
            price_target: None,
            recommendations: None,
            consecutive_provider_degraded_cycles: 2,
        };

        let data_outcome = HydratedConsensusFetch::Success(ConsensusOutcome::Data(data_evidence(
            symbol, 99, // pretend the upstream evidence carries a non-zero counter
        )));
        let next = apply_consensus_half_life_policy(symbol, &data_outcome, Some(&prior_stub));
        assert!(matches!(next.status, EnrichmentStatus::Available));
        let payload = next.payload.expect("Data should carry a payload");
        assert_eq!(
            payload.consecutive_provider_degraded_cycles, 0,
            "Data outcome must reset counter regardless of upstream value"
        );
    }
}
