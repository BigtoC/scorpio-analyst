//! Pack-driven pipeline construction.
//!
//! Phase 7 extracts the graph-wiring logic that previously lived inside
//! `workflow::pipeline::runtime::build_graph` so callers can hand it a
//! fully-resolved [`AnalysisPackManifest`] directly instead of a pack id
//! string. The pipeline topology (preflight → analyst fan-out → analyst
//! sync → debate → trader → risk → fund manager) is stable across packs;
//! what varies is the analyst set, driven by `pack.required_inputs`
//! through the [`AnalystRegistry`].
//!
//! # When to call `from_pack` vs `new`
//!
//! - [`TradingPipeline::new`] stays the primary caller-facing API and
//!   already resolves the pack internally from `config.analysis_pack`.
//! - [`TradingPipeline::from_pack`] is the pack-first entry point used
//!   when the caller has already resolved the pack manifest (tests,
//!   future runtime-loaded packs, feature-flagged experimental packs).
use std::sync::Arc;
use std::time::Duration;

use graph_flow::{Graph, fanout::FanOutTask};

use super::pipeline::TradingPipeline;
use super::pipeline::constants::TASKS;
use super::pipeline::runtime::build_analyst_tasks;
use super::snapshot::SnapshotStore;
use super::tasks::{
    AggressiveRiskTask, AnalystSyncTask, BearishResearcherTask, BullishResearcherTask,
    ConservativeRiskTask, DebateModeratorTask, FundManagerTask, KEY_DEBATE_ROUND,
    KEY_MAX_DEBATE_ROUNDS, KEY_MAX_RISK_ROUNDS, KEY_RISK_ROUND, NeutralRiskTask, PreflightTask,
    RiskModeratorTask, TraderTask,
};
use crate::agents::analyst::AnalystRegistry;
use crate::analysis_packs::AnalysisPackManifest;
use crate::config::Config;
use crate::data::{FinnhubClient, FredClient, YFinanceClient};
use crate::providers::factory::CompletionModelHandle;

/// Build a fully-wired pipeline graph from a resolved pack manifest.
///
/// `pack.required_inputs` drives the analyst fan-out; the remaining
/// topology is fixed. `registry` is consulted to decide which analyst
/// tasks can actually be spawned — unknown ids are silently dropped,
/// matching the graceful-degradation contract in
/// `workflow::tasks::analyst::AnalystSyncTask`.
///
/// The positional signature carries every piece of shared state the
/// pipeline needs; grouping into a struct would just push the argument
/// count around, so the clippy lint is silenced here.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_graph_from_pack(
    pack: &AnalysisPackManifest,
    config: Arc<Config>,
    registry: &AnalystRegistry,
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
        pack.id.to_string(),
    );
    graph.add_task(Arc::new(preflight));

    let analyst_tasks = build_analyst_tasks(
        registry,
        &pack.required_inputs,
        finnhub,
        fred,
        yfinance,
        quick_handle,
        &config.llm,
    );
    let fan_out = FanOutTask::new(TASKS.analyst_fan_out, analyst_tasks);
    graph.add_task(fan_out);
    graph.add_edge(TASKS.preflight, TASKS.analyst_fan_out);

    let analyst_sync = AnalystSyncTask::with_yfinance(
        Arc::clone(&snapshot_store),
        yfinance.clone(),
        Duration::from_secs(config.llm.valuation_fetch_timeout_secs),
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

/// Dependencies handed to [`TradingPipeline::from_pack`].
///
/// Grouping these into a struct keeps the constructor signature short and
/// makes it obvious which pieces of state are shared vs owned by the
/// pipeline.
#[derive(Debug)]
pub struct PipelineDeps {
    pub config: Config,
    pub finnhub: FinnhubClient,
    pub fred: FredClient,
    pub yfinance: YFinanceClient,
    pub snapshot_store: SnapshotStore,
    pub quick_handle: CompletionModelHandle,
    pub deep_handle: CompletionModelHandle,
}

impl TradingPipeline {
    /// Construct a pipeline from a pre-resolved pack manifest.
    ///
    /// Callers that go through `scorpio-cli` still use
    /// [`TradingPipeline::new`]; `from_pack` is intended for tests,
    /// feature-flagged experiments, and future external pack loaders
    /// that have already resolved the manifest themselves.
    #[must_use]
    pub fn from_pack(pack: &AnalysisPackManifest, deps: PipelineDeps) -> Self {
        let PipelineDeps {
            config,
            finnhub,
            fred,
            yfinance,
            snapshot_store,
            quick_handle,
            deep_handle,
        } = deps;
        let config = Arc::new(config);
        let snapshot_store = Arc::new(snapshot_store);
        let registry = AnalystRegistry::equity_baseline();
        let graph = build_graph_from_pack(
            pack,
            Arc::clone(&config),
            &registry,
            &finnhub,
            &fred,
            &yfinance,
            Arc::clone(&snapshot_store),
            &quick_handle,
            &deep_handle,
        );
        Self::__from_parts(
            config,
            finnhub,
            fred,
            yfinance,
            snapshot_store,
            quick_handle,
            deep_handle,
            graph,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::analyst::AnalystId;
    use crate::analysis_packs::{PackId, resolve_pack};

    #[test]
    fn baseline_pack_for_inputs_yields_four_canonical_analysts() {
        let pack = resolve_pack(PackId::Baseline);
        let registry = AnalystRegistry::equity_baseline();
        let ids = registry.for_inputs(pack.required_inputs.iter().map(String::as_str));
        assert_eq!(
            ids,
            vec![
                AnalystId::Fundamental,
                AnalystId::Sentiment,
                AnalystId::News,
                AnalystId::Technical,
            ],
            "baseline pack must dispatch to exactly the four canonical analysts in input order"
        );
    }

    #[test]
    fn crypto_digital_asset_pack_is_unselectable_but_resolvable() {
        let err = "crypto_digital_asset"
            .parse::<PackId>()
            .expect_err("stub pack must not be selectable via config");
        assert!(err.contains("unknown analysis pack"));
        // Direct registry access still works for test / future-flag callers.
        let pack = resolve_pack(PackId::CryptoDigitalAsset);
        assert_eq!(pack.id, PackId::CryptoDigitalAsset);
    }

    #[test]
    fn crypto_digital_asset_for_inputs_yields_empty_on_equity_baseline_registry() {
        // The stub pack's `required_inputs` name crypto analysts; the equity-
        // baseline registry does not register them, so `for_inputs` returns
        // an empty Vec — the graph builder will spawn zero analyst tasks,
        // which is the desired safety fallback until the crypto pack lands.
        let pack = resolve_pack(PackId::CryptoDigitalAsset);
        let registry = AnalystRegistry::equity_baseline();
        let ids = registry.for_inputs(pack.required_inputs.iter().map(String::as_str));
        assert!(
            ids.is_empty(),
            "equity-baseline registry must produce no analysts for the crypto stub pack"
        );
    }
}
