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
        catalysts::{
            CatalystCalendarProvider, CatalystEvent, SecEdgar8kProvider, Tier1CatalystProvider,
            Tier2CatalystProvider,
        },
        estimates::{
            ConsensusEvidence, ConsensusOutcome, EstimatesProvider, YFinanceEstimatesProvider,
        },
        events::{EventNewsEvidence, EventNewsProvider, FinnhubEventNewsProvider},
    },
    data::{FinnhubClient, FredClient, SecEdgarClient, YFinanceClient},
    domain::Symbol,
    error::TradingError,
    providers::factory::CompletionModelHandle,
    rate_limit::SharedRateLimiter,
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

/// Staleness window for reusing prior consensus bookkeeping from phase-1 snapshots.
const CONSENSUS_MEMORY_MAX_AGE_DAYS: i64 = 30;

use super::{MAX_PIPELINE_STEPS, TradingPipeline, constants::TASKS, errors};

pub(super) fn canonicalize_runtime_symbol(symbol: &str) -> Result<Symbol, TradingError> {
    Ok(crate::data::resolve_symbol(symbol)?.symbol)
}

pub(crate) fn build_catalyst_provider(
    finnhub: &FinnhubClient,
    fred: &FredClient,
    yfinance: &YFinanceClient,
    source_timeout: std::time::Duration,
) -> Arc<dyn CatalystCalendarProvider> {
    let tier1 = Tier1CatalystProvider::with_timeout(
        finnhub.clone(),
        fred.clone(),
        yfinance.clone(),
        source_timeout,
    );

    match SecEdgarClient::new(SharedRateLimiter::new("sec-edgar", 10)) {
        Ok(edgar_client) => {
            info!("catalyst provider: Tier 2 (Finnhub + FRED + yfinance + SEC EDGAR)");
            Arc::new(Tier2CatalystProvider {
                tier1,
                sec_edgar: SecEdgar8kProvider::new(edgar_client),
                source_timeout,
            })
        }
        Err(reason) => {
            info!(reason = %reason, "falling back to Tier 1 catalyst provider");
            Arc::new(tier1)
        }
    }
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
    state.enrichment_catalysts = EnrichmentState::default();
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
    state.audit_status = Default::default();
    state.audit_report = None;
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

/// Guard that validates Copilot authentication before the pipeline preflight task
/// runs, using the real [`crate::providers::factory::copilot_auth::fetch_github_identity`]
/// to verify the live GitHub identity matches the stored binding.
///
/// This is a thin wrapper around [`validate_copilot_auth_before_preflight_with`] that
/// resolves `token_dir` from the user settings and passes the real identity-fetch
/// function as the injectable seam.
pub(super) async fn validate_copilot_auth_before_preflight_if_configured(
    cfg: &Config,
) -> anyhow::Result<()> {
    if cfg.llm.quick_thinking_provider != "copilot" && cfg.llm.deep_thinking_provider != "copilot" {
        return Ok(());
    }
    let token_dir = crate::settings::copilot_token_dir()
        .map_err(|e| anyhow::anyhow!("Copilot token dir: {e}"))?;
    validate_copilot_auth_before_preflight_with(cfg, &token_dir, |token| {
        Box::pin(crate::providers::factory::copilot_auth::fetch_github_identity(token))
    })
    .await
}

/// Validate Copilot authentication before the pipeline preflight task runs.
///
/// The `fetch_identity` parameter is an injectable seam for testing. In production
/// call [`validate_copilot_auth_before_preflight_if_configured`] which wires in the
/// real [`crate::providers::factory::copilot_auth::fetch_github_identity`].
///
/// If neither `quick_thinking_provider` nor `deep_thinking_provider` is `"copilot"`,
/// this returns `Ok(())` immediately without touching the token directory.
pub(super) async fn validate_copilot_auth_before_preflight_with<F>(
    cfg: &Config,
    token_dir: &std::path::Path,
    fetch_identity: F,
) -> anyhow::Result<()>
where
    F: for<'a> Fn(
        &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        crate::providers::factory::copilot_auth::GitHubIdentity,
                        TradingError,
                    >,
                > + Send
                + 'a,
        >,
    >,
{
    use crate::providers::factory::copilot_auth;

    if cfg.llm.quick_thinking_provider != "copilot" && cfg.llm.deep_thinking_provider != "copilot" {
        return Ok(());
    }

    crate::settings::verify_copilot_token_dir_secure(token_dir)
        .map_err(|e| anyhow::anyhow!("Copilot token directory rejected: {e}"))?;

    let binding = copilot_auth::read_binding(token_dir)
        .map_err(|e| anyhow::anyhow!("Copilot identity binding: {e}"))?;

    let record = copilot_auth::read_api_key_record(token_dir)
        .map_err(|e| anyhow::anyhow!("Copilot api-key cache: {e}"))?;
    copilot_auth::validate_copilot_runtime_base(&record)
        .map_err(|e| anyhow::anyhow!("Copilot runtime base: {e}"))?;

    let access = copilot_auth::read_access_token(token_dir)
        .map_err(|e| anyhow::anyhow!("Copilot access token: {e}"))?;

    let identity = fetch_identity(&access)
        .await
        .map_err(|e| anyhow::anyhow!("Copilot identity fetch: {e}"))?;

    copilot_auth::validate_scope(&identity.scopes)
        .map_err(|e| anyhow::anyhow!("Copilot scope validation: {e}"))?;

    if identity.id != binding.github_id {
        return Err(anyhow::anyhow!(
            "Copilot live identity (id={}) does not match bound GitHub account (id={}); \
             rerun `scorpio setup` to re-authorize",
            identity.id,
            binding.github_id
        ));
    }

    Ok(())
}

/// Run a full analysis cycle for the given initial state.
///
/// 1. Resets per-cycle outputs on the provided state.
/// 2. Canonicalizes the runtime symbol before any best-effort prefetch.
/// 3. Seeds a fresh in-memory session with the serialized `TradingState`.
/// 4. Runs the `FlowRunner` loop until the pipeline completes.
/// 5. Deserializes and returns the final `TradingState`.
#[instrument(skip(pipeline, initial_state), fields(symbol = %initial_state.asset_symbol, date = %initial_state.target_date))]
pub async fn run_analysis_cycle(
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

    validate_copilot_auth_before_preflight_if_configured(&pipeline.config)
        .await
        .map_err(TradingError::Config)?;

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

    let prior_consensus_payload = match prior_consensus_payload {
        Some(payload) if payload.symbol.eq_ignore_ascii_case(&symbol) => Some(payload),
        Some(payload) => {
            info!(
                current_symbol = %symbol,
                prior_symbol = %payload.symbol,
                "discarding in-memory consensus payload for a different symbol"
            );
            load_prior_consensus_payload(&pipeline.snapshot_store, &symbol).await
        }
        None => match pipeline
            .snapshot_store
            .load_prior_consensus_for_symbol(&symbol, CONSENSUS_MEMORY_MAX_AGE_DAYS)
            .await
        {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    symbol = %symbol,
                    error = %error,
                    "prior consensus lookup failed; continuing without half-life history"
                );
                None
            }
        },
    };

    let need_price = initial_state.current_price.is_none();
    let need_vix = initial_state.market_volatility().is_none();
    let (price_result, vix_result, news_result, catalysts_result) = {
        use crate::agents::analyst::prefetch_analyst_news;
        use crate::data::YFinanceNewsProvider;
        let yfinance_news_provider = YFinanceNewsProvider::new(&pipeline.yfinance);
        let fetch_timeout =
            std::time::Duration::from_secs(pipeline.config.enrichment.fetch_timeout_secs);
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
            prefetch_analyst_news(&pipeline.finnhub, &yfinance_news_provider, &symbol),
            hydrate_catalysts(
                pipeline.catalyst_provider.as_ref(),
                &symbol,
                &date,
                fetch_timeout
            ),
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
    initial_state.enrichment_catalysts = catalysts_result;

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

async fn load_prior_consensus_payload(
    snapshot_store: &SnapshotStore,
    symbol: &str,
) -> Option<ConsensusEvidence> {
    match snapshot_store
        .load_prior_consensus_for_symbol(symbol, CONSENSUS_MEMORY_MAX_AGE_DAYS)
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                symbol = %symbol,
                error = %error,
                "prior consensus lookup failed; continuing without half-life history"
            );
            None
        }
    }
}

// ─── Enrichment hydration helpers ────────────────────────────────────────────

/// Fetch catalyst-calendar enrichment with a timeout boundary.
///
/// Returns an `EnrichmentState` with:
/// - `payload: Some(events)` when the fetch ran (even if all sources failed,
///   returning `Some(vec![])` — distinguishable from "not attempted").
/// - `payload: None` only when this function is never called (skipped in
///   the join! block), which is not the case in the current wiring.
///
/// All per-source failures are absorbed by `Tier1CatalystProvider`'s
/// fail-soft `try_*` helpers and emitted as `tracing::warn!`.
async fn hydrate_catalysts(
    provider: &dyn CatalystCalendarProvider,
    symbol: &str,
    as_of_date: &str,
    _timeout: std::time::Duration,
) -> EnrichmentState<Vec<CatalystEvent>> {
    const HORIZON_DAYS: u32 = 30;
    match provider
        .fetch_catalysts(symbol, as_of_date, HORIZON_DAYS)
        .await
    {
        Ok(events) if events.is_empty() => {
            info!(
                symbol,
                "catalyst-calendar enrichment: no upcoming catalysts"
            );
            EnrichmentState {
                status: EnrichmentStatus::Available,
                payload: Some(vec![]),
            }
        }
        Ok(events) => {
            info!(
                symbol,
                count = events.len(),
                "catalyst-calendar enrichment: available"
            );
            EnrichmentState {
                status: EnrichmentStatus::Available,
                payload: Some(events),
            }
        }
        Err(e) => {
            info!(symbol, error = %e, "catalyst-calendar enrichment: date arithmetic error (fail-open)");
            EnrichmentState {
                status: EnrichmentStatus::FetchFailed(e.to_string()),
                payload: Some(vec![]),
            }
        }
    }
}

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
                payload: prior_payload.cloned(),
            }
        }
        HydratedConsensusFetch::TimedOut => {
            info!(
                symbol,
                "consensus-estimates enrichment: timed out (fail-open)"
            );
            EnrichmentState {
                status: EnrichmentStatus::FetchFailed("timeout".to_owned()),
                payload: prior_payload.cloned(),
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
mod preflight_copilot_guard_tests {
    use super::*;
    use crate::providers::factory::copilot_auth;

    fn sample_llm_config() -> crate::config::LlmConfig {
        crate::config::LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 1,
            max_risk_rounds: 1,
            analyst_timeout_secs: 30,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 1,
            retry_base_delay_ms: 100,
        }
    }

    fn sample_config_with_llm(llm: crate::config::LlmConfig) -> Config {
        Config {
            llm,
            trading: crate::config::TradingConfig::default(),
            api: Default::default(),
            providers: Default::default(),
            storage: Default::default(),
            rate_limits: Default::default(),
            enrichment: Default::default(),
            analysis_pack: "baseline".to_owned(),
        }
    }

    fn write_full_copilot_cache(token_dir: &std::path::Path, github_id: u64) {
        std::fs::create_dir_all(token_dir).unwrap();
        std::fs::write(token_dir.join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            token_dir.join("api-key.json"),
            r#"{"token":"tid_test","expires_at":4102444800,"endpoints":{"api":"https://api.githubcopilot.com"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                token_dir.join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                token_dir.join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        copilot_auth::write_binding(
            token_dir,
            &copilot_auth::ScorpioIdentityBinding {
                github_id,
                github_login: "octocat".to_owned(),
                written_at: 0,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn preflight_guard_skips_when_copilot_not_selected() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = sample_config_with_llm(sample_llm_config());
        validate_copilot_auth_before_preflight_with(&cfg, temp.path(), |_token| {
            Box::pin(async { panic!("should not fetch identity when Copilot is not selected") })
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn preflight_guard_rejects_when_live_github_identity_mismatches_binding() {
        let temp = tempfile::tempdir().unwrap();
        let token_dir = temp.path().join("github_copilot");
        write_full_copilot_cache(&token_dir, 42);
        let mut llm = sample_llm_config();
        llm.quick_thinking_provider = "copilot".to_owned();
        let cfg = sample_config_with_llm(llm);

        // On non-Unix the token dir security check is a no-op; set up
        // a proper 0o700 dir on Unix so verify_copilot_token_dir_secure passes.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let err = validate_copilot_auth_before_preflight_with(&cfg, &token_dir, |_token| {
            Box::pin(async {
                Ok(copilot_auth::GitHubIdentity {
                    id: 99,
                    login: "wrong-user".to_owned(),
                    scopes: vec!["read:user".to_owned()],
                })
            })
        })
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("bound GitHub account"),
            "expected 'bound GitHub account' in: {err:#}"
        );
    }

    #[tokio::test]
    async fn preflight_guard_rejects_when_live_scopes_exceed_allowed_set() {
        let temp = tempfile::tempdir().unwrap();
        let token_dir = temp.path().join("github_copilot");
        write_full_copilot_cache(&token_dir, 42);
        let mut llm = sample_llm_config();
        llm.quick_thinking_provider = "copilot".to_owned();
        let cfg = sample_config_with_llm(llm);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let err = validate_copilot_auth_before_preflight_with(&cfg, &token_dir, |_token| {
            Box::pin(async {
                Ok(copilot_auth::GitHubIdentity {
                    id: 42,
                    login: "octocat".to_owned(),
                    scopes: vec!["read:user".to_owned(), "repo".to_owned()],
                })
            })
        })
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("scope"),
            "expected 'scope' in: {err:#}"
        );
    }

    #[tokio::test]
    async fn preflight_guard_calls_identity_exactly_once() {
        let temp = tempfile::tempdir().unwrap();
        let token_dir = temp.path().join("github_copilot");
        write_full_copilot_cache(&token_dir, 42);
        let mut llm = sample_llm_config();
        llm.quick_thinking_provider = "copilot".to_owned();
        let cfg = sample_config_with_llm(llm);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let _ = validate_copilot_auth_before_preflight_with(&cfg, &token_dir, move |_token| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(copilot_auth::GitHubIdentity {
                    id: 42,
                    login: "octocat".to_owned(),
                    scopes: vec!["read:user".to_owned()],
                })
            })
        })
        .await;

        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
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

    #[test]
    fn failed_fetch_preserves_prior_payload_for_future_half_life_cycles() {
        let symbol = "AAPL";
        let prior = data_evidence(symbol, 2);

        let next = apply_consensus_half_life_policy(
            symbol,
            &HydratedConsensusFetch::Failed("price target down".to_owned()),
            Some(&prior),
        );

        assert!(
            matches!(next.status, EnrichmentStatus::FetchFailed(ref reason) if reason == "price target down"),
            "failed fetch must surface the operational failure reason, got {:?}",
            next.status
        );
        assert_eq!(
            next.payload.as_ref(),
            Some(&prior),
            "failed fetch must preserve the prior payload so the degraded counter does not reset"
        );
    }

    #[test]
    fn timeout_preserves_prior_payload_for_future_half_life_cycles() {
        let symbol = "AAPL";
        let prior = data_evidence(symbol, 2);

        let next = apply_consensus_half_life_policy(
            symbol,
            &HydratedConsensusFetch::TimedOut,
            Some(&prior),
        );

        assert!(
            matches!(next.status, EnrichmentStatus::FetchFailed(ref reason) if reason == "timeout"),
            "timed-out fetch must surface timeout status, got {:?}",
            next.status
        );
        assert_eq!(
            next.payload.as_ref(),
            Some(&prior),
            "timeout must preserve the prior payload so the degraded counter does not reset"
        );
    }
}

#[cfg(test)]
mod catalyst_hydration_tests {
    use super::*;
    use std::sync::Arc;

    struct StaticCatalystProvider {
        result: Arc<Vec<CatalystEvent>>,
    }

    #[async_trait::async_trait]
    impl CatalystCalendarProvider for StaticCatalystProvider {
        async fn fetch_catalysts(
            &self,
            _symbol: &str,
            _as_of_date: &str,
            _horizon_days: u32,
        ) -> Result<Vec<CatalystEvent>, TradingError> {
            Ok(self.result.as_ref().clone())
        }
    }

    #[tokio::test]
    async fn hydrate_catalysts_empty_success_marks_quiet_window_as_available() {
        let provider = StaticCatalystProvider {
            result: Arc::new(vec![]),
        };

        let state = hydrate_catalysts(
            &provider,
            "AAPL",
            "2026-01-15",
            std::time::Duration::from_secs(1),
        )
        .await;

        assert_eq!(state.status, EnrichmentStatus::Available);
        assert_eq!(state.payload, Some(vec![]));
    }
}
