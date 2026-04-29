use chrono::{TimeZone, Utc};

use crate::{
    agents::{
        analyst::equity::{
            build_fundamental_system_prompt, build_news_system_prompt,
            build_sentiment_system_prompt, build_technical_system_prompt,
        },
        fund_manager::build_prompt_context as build_fund_manager_prompt_context,
        researcher::render_researcher_system_prompt,
        risk::{DualRiskStatus, render_risk_system_prompt},
        trader::build_prompt_context_for_test as build_trader_prompt_context,
    },
    data::traits::options::{IvTermPoint, OptionsOutcome, OptionsSnapshot},
    state::{
        DataCoverageReport, DebateMessage, EvidenceKind, EvidenceRecord, EvidenceSource,
        FundamentalData, MarketVolatilityData, NewsData, ProvenanceSummary, RiskLevel, RiskReport,
        SentimentData, TechnicalData, TechnicalOptionsContext, TradeAction, TradeProposal,
        TradingState, VixRegime, VixTrend,
    },
    workflow::Role,
};

const FIXTURE_TICKER: &str = "AAPL";
const FIXTURE_DATE: &str = "2026-04-25";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptRenderScenario {
    AllInputsPresent,
    ZeroDebate,
    ZeroRisk,
    MissingAnalystData,
}

/// Fully rendered prompt output for a role/scenario pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRenderOutput {
    pub system_prompt: String,
    pub user_prompt: Option<String>,
}

/// Render the baseline system prompt bytes for one role under one canned test
/// scenario.
#[must_use]
pub fn render_baseline_prompt_for_role(role: Role, scenario: PromptRenderScenario) -> String {
    render_prompt_output_for_role(role, scenario).system_prompt
}

/// Render the full prompt output for one role under one canned test scenario.
#[must_use]
pub fn render_prompt_output_for_role(
    role: Role,
    scenario: PromptRenderScenario,
) -> PromptRenderOutput {
    render_prompt_output(role, build_state(scenario), scenario)
}

// `render_legacy_fallback_system_prompt_for_role` and
// `render_blank_slot_fallback_system_prompt_for_role` were removed in Phase 7
// of the prompt-bundle centralization migration. Both helpers exercised
// renderer code paths that no longer exist:
//
// - the legacy-fallback helper set `state.analysis_runtime_policy = None`
//   to force the renderer to use a hardcoded `legacy_template` constant,
//   but the renderer now requires `&RuntimePolicy` and has no fallback
//   branch; preflight is the sole writer of the runtime policy.
// - the blank-slot-fallback helper blanked the active role's bundle slot to
//   force the same legacy fallback, which is also gone.
//
// `validate_active_pack_completeness` now rejects packs whose required
// slots are empty *before* the renderer runs, so the conditions both
// helpers used to simulate are unreachable in production. The byte-
// equivalence assertions they powered have been removed alongside the
// helpers themselves.

fn render_prompt_output(
    role: Role,
    state: TradingState,
    scenario: PromptRenderScenario,
) -> PromptRenderOutput {
    let analyst_policy = || {
        state
            .analysis_runtime_policy
            .as_ref()
            .expect("test fixture must hydrate runtime policy")
    };
    match role {
        Role::FundamentalAnalyst => PromptRenderOutput {
            system_prompt: build_fundamental_system_prompt(
                &state.asset_symbol,
                &state.target_date,
                analyst_policy(),
            ),
            user_prompt: None,
        },
        Role::SentimentAnalyst => PromptRenderOutput {
            system_prompt: build_sentiment_system_prompt(
                &state.asset_symbol,
                &state.target_date,
                analyst_policy(),
            ),
            user_prompt: None,
        },
        Role::NewsAnalyst => PromptRenderOutput {
            system_prompt: build_news_system_prompt(
                &state.asset_symbol,
                &state.target_date,
                analyst_policy(),
            ),
            user_prompt: None,
        },
        Role::TechnicalAnalyst => PromptRenderOutput {
            system_prompt: build_technical_system_prompt(
                &state.asset_symbol,
                &state.target_date,
                analyst_policy(),
                true, // tool-available variant for deterministic fixtures
            ),
            user_prompt: None,
        },
        Role::BullishResearcher => PromptRenderOutput {
            system_prompt: render_researcher_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.bullish_researcher.as_ref(),
            ),
            user_prompt: None,
        },
        Role::BearishResearcher => PromptRenderOutput {
            system_prompt: render_researcher_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.bearish_researcher.as_ref(),
            ),
            user_prompt: None,
        },
        Role::DebateModerator => PromptRenderOutput {
            system_prompt: render_researcher_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.debate_moderator.as_ref(),
            ),
            user_prompt: None,
        },
        Role::Trader => {
            let context =
                build_trader_prompt_context(&state, &state.asset_symbol, &state.target_date);
            PromptRenderOutput {
                system_prompt: context.system_prompt,
                user_prompt: Some(context.user_prompt),
            }
        }
        Role::AggressiveRisk => PromptRenderOutput {
            system_prompt: render_risk_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.aggressive_risk.as_ref(),
            ),
            user_prompt: None,
        },
        Role::ConservativeRisk => PromptRenderOutput {
            system_prompt: render_risk_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.conservative_risk.as_ref(),
            ),
            user_prompt: None,
        },
        Role::NeutralRisk => PromptRenderOutput {
            system_prompt: render_risk_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.neutral_risk.as_ref(),
            ),
            user_prompt: None,
        },
        Role::RiskModerator => PromptRenderOutput {
            system_prompt: render_risk_system_prompt(
                state
                    .analysis_runtime_policy
                    .as_ref()
                    .expect("test fixture must hydrate runtime policy"),
                &state,
                |bundle| bundle.risk_moderator.as_ref(),
            ),
            user_prompt: None,
        },
        Role::FundManager => {
            let dual_risk = DualRiskStatus::from_reports_with_topology(
                state.conservative_risk_report.as_ref(),
                state.neutral_risk_report.as_ref(),
                !matches!(scenario, PromptRenderScenario::ZeroRisk),
            );
            let (system_prompt, user_prompt) = build_fund_manager_prompt_context(
                &state,
                &state.asset_symbol,
                &state.target_date,
                dual_risk,
            );
            PromptRenderOutput {
                system_prompt,
                user_prompt: Some(user_prompt),
            }
        }
    }
}

#[must_use]
pub fn canonical_fixture_identity() -> (&'static str, &'static str) {
    (FIXTURE_TICKER, FIXTURE_DATE)
}

// `blank_selected_slot` was removed in Phase 10 of the prompt-bundle
// centralization migration — it powered the now-deleted
// `render_blank_slot_fallback_system_prompt_for_role` helper. Production
// preflight rejects packs with empty required slots, so the simulated
// "blank slot" failure mode no longer has a callable surface in tests.

fn build_state(scenario: PromptRenderScenario) -> TradingState {
    let mut state = TradingState::new(FIXTURE_TICKER, FIXTURE_DATE);
    super::runtime_policy::with_baseline_runtime_policy(&mut state);
    state.analysis_pack_name = Some("baseline".to_owned());
    state.data_coverage = Some(DataCoverageReport {
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        missing_inputs: Vec::new(),
    });
    state.set_market_volatility(sample_market_volatility());

    match scenario {
        PromptRenderScenario::AllInputsPresent => populate_all_inputs_present(&mut state),
        PromptRenderScenario::ZeroDebate => populate_zero_debate(&mut state),
        PromptRenderScenario::ZeroRisk => populate_zero_risk(&mut state),
        PromptRenderScenario::MissingAnalystData => populate_missing_analyst_data(&mut state),
    }

    state.provenance_summary = Some(ProvenanceSummary {
        providers_used: providers_used_for_state(&state),
    });

    state
}

fn populate_all_inputs_present(state: &mut TradingState) {
    state.set_fundamental_metrics(sample_fundamental_data());
    state.set_market_sentiment(sample_sentiment_data());
    state.set_macro_news(sample_news_data());
    state.set_technical_indicators(sample_technical_data());
    state.set_evidence_fundamental(EvidenceRecord {
        kind: EvidenceKind::Fundamental,
        payload: sample_fundamental_data(),
        sources: vec![sample_evidence_source("finnhub", &["fundamentals"])],
        quality_flags: vec![],
    });
    state.set_evidence_sentiment(EvidenceRecord {
        kind: EvidenceKind::Sentiment,
        payload: sample_sentiment_data(),
        sources: vec![sample_evidence_source(
            "finnhub",
            &["company_news_sentiment_inputs"],
        )],
        quality_flags: vec![],
    });
    state.set_evidence_news(EvidenceRecord {
        kind: EvidenceKind::News,
        payload: sample_news_data(),
        sources: vec![
            sample_evidence_source("finnhub", &["company_news"]),
            sample_evidence_source("fred", &["macro_indicators"]),
        ],
        quality_flags: vec![],
    });
    state.set_evidence_technical(EvidenceRecord {
        kind: EvidenceKind::Technical,
        payload: sample_technical_data(),
        sources: vec![sample_evidence_source("yfinance", &["ohlcv"])],
        quality_flags: vec![],
    });
    state.consensus_summary = Some(
        "Hold - strongest bull evidence is growth, strongest bear evidence is rates, unresolved uncertainty is demand durability."
            .to_owned(),
    );
    state.trader_proposal = Some(sample_trade_proposal());
    state.aggressive_risk_report = Some(sample_risk_report(
        RiskLevel::Aggressive,
        "Aggressive view: upside remains actionable with tight monitoring.",
        false,
    ));
    state.conservative_risk_report = Some(sample_risk_report(
        RiskLevel::Conservative,
        "Conservative view: controls are acceptable but valuation leaves less room for error.",
        false,
    ));
    state.neutral_risk_report = Some(sample_risk_report(
        RiskLevel::Neutral,
        "Neutral view: risk and reward are balanced enough for a Hold stance.",
        false,
    ));
    state.risk_discussion_history = vec![
        DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: "The setup still supports measured upside exposure.".to_owned(),
        },
        DebateMessage {
            role: "risk_moderator".to_owned(),
            content: "Consensus: controls are adequate, but valuation requires discipline."
                .to_owned(),
        },
    ];
    state.current_price = Some(182.34);
}

fn populate_zero_debate(state: &mut TradingState) {
    populate_all_inputs_present(state);
    state.consensus_summary = None;
    state.debate_history.clear();
}

fn populate_zero_risk(state: &mut TradingState) {
    populate_all_inputs_present(state);
    state.aggressive_risk_report = None;
    state.conservative_risk_report = None;
    state.neutral_risk_report = None;
    state.risk_discussion_history.clear();
}

fn populate_missing_analyst_data(state: &mut TradingState) {
    state.consensus_summary = None;
    state.current_price = Some(182.34);
    state.data_coverage = Some(DataCoverageReport {
        required_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
        missing_inputs: vec![
            "fundamentals".to_owned(),
            "sentiment".to_owned(),
            "news".to_owned(),
            "technical".to_owned(),
        ],
    });
    state.trader_proposal = Some(sample_trade_proposal());
    state.aggressive_risk_report = Some(sample_risk_report(
        RiskLevel::Aggressive,
        "Aggressive view: upside remains actionable with tight monitoring.",
        false,
    ));
    state.conservative_risk_report = Some(sample_risk_report(
        RiskLevel::Conservative,
        "Conservative view: controls are acceptable but valuation leaves less room for error.",
        false,
    ));
    state.neutral_risk_report = Some(sample_risk_report(
        RiskLevel::Neutral,
        "Neutral view: risk and reward are balanced enough for a Hold stance.",
        false,
    ));
    state.risk_discussion_history = vec![
        DebateMessage {
            role: "aggressive_risk".to_owned(),
            content: "The setup still supports measured upside exposure.".to_owned(),
        },
        DebateMessage {
            role: "risk_moderator".to_owned(),
            content: "Consensus: controls are adequate, but valuation requires discipline."
                .to_owned(),
        },
    ];
}

fn sample_evidence_source(provider: &str, datasets: &[&str]) -> EvidenceSource {
    EvidenceSource {
        provider: provider.to_owned(),
        datasets: datasets
            .iter()
            .map(|dataset| (*dataset).to_owned())
            .collect(),
        fetched_at: Utc
            .with_ymd_and_hms(2026, 4, 25, 0, 0, 0)
            .single()
            .expect("fixed evidence timestamp should be valid"),
        effective_at: None,
        url: None,
        citation: None,
    }
}

fn providers_used_for_state(state: &TradingState) -> Vec<String> {
    let mut providers = Vec::new();
    if let Some(record) = state.evidence_fundamental() {
        providers.extend(record.sources.iter().map(|source| source.provider.clone()));
    }
    if let Some(record) = state.evidence_sentiment() {
        providers.extend(record.sources.iter().map(|source| source.provider.clone()));
    }
    if let Some(record) = state.evidence_news() {
        providers.extend(record.sources.iter().map(|source| source.provider.clone()));
    }
    if let Some(record) = state.evidence_technical() {
        providers.extend(record.sources.iter().map(|source| source.provider.clone()));
    }
    providers.sort_unstable();
    providers.dedup();
    providers
}

fn sample_fundamental_data() -> FundamentalData {
    FundamentalData {
        revenue_growth_pct: Some(12.5),
        pe_ratio: Some(24.5),
        eps: Some(6.05),
        current_ratio: Some(1.1),
        debt_to_equity: Some(1.8),
        gross_margin: Some(0.44),
        net_income: Some(97_000_000_000.0),
        insider_transactions: Vec::new(),
        summary: "Revenue growth remains healthy while margins stay resilient.".to_owned(),
    }
}

fn sample_sentiment_data() -> SentimentData {
    SentimentData {
        overall_score: 0.42,
        source_breakdown: Vec::new(),
        engagement_peaks: Vec::new(),
        summary: "Market narrative is constructive but no longer euphoric.".to_owned(),
    }
}

fn sample_news_data() -> NewsData {
    NewsData {
        articles: Vec::new(),
        macro_events: Vec::new(),
        summary: "Recent news flow is mixed but not thesis-breaking.".to_owned(),
    }
}

fn sample_technical_data() -> TechnicalData {
    TechnicalData {
        rsi: Some(55.0),
        macd: None,
        atr: Some(3.2),
        sma_20: Some(178.0),
        sma_50: Some(171.0),
        ema_12: Some(179.2),
        ema_26: Some(176.4),
        bollinger_upper: Some(185.0),
        bollinger_lower: Some(171.5),
        support_level: Some(176.0),
        resistance_level: Some(184.5),
        volume_avg: Some(72_000_000.0),
        summary: "Trend is constructive with price holding above key moving averages.".to_owned(),
        options_summary: Some(
            "Near-term IV remains elevated, but the front-month term structure is orderly."
                .to_owned(),
        ),
        options_context: Some(TechnicalOptionsContext::Available {
            outcome: OptionsOutcome::Snapshot(OptionsSnapshot {
                spot_price: 180.0,
                atm_iv: 0.28,
                iv_term_structure: vec![IvTermPoint {
                    expiration: FIXTURE_DATE.to_owned(),
                    atm_iv: 0.28,
                }],
                put_call_volume_ratio: 1.1,
                put_call_oi_ratio: 1.0,
                max_pain_strike: 180.0,
                near_term_expiration: FIXTURE_DATE.to_owned(),
                near_term_strikes: vec![],
            }),
        }),
    }
}

fn sample_market_volatility() -> MarketVolatilityData {
    MarketVolatilityData {
        vix_level: 18.5,
        vix_sma_20: 17.2,
        vix_trend: VixTrend::Rising,
        vix_regime: VixRegime::Normal,
        fetched_at: FIXTURE_DATE.to_owned(),
    }
}

fn sample_trade_proposal() -> TradeProposal {
    TradeProposal {
        action: TradeAction::Hold,
        target_price: 190.0,
        stop_loss: 170.0,
        confidence: 0.62,
        rationale: "Hold while waiting for valuation and momentum to align more clearly."
            .to_owned(),
        valuation_assessment: Some(
            "Fair value to slightly rich versus current assumptions.".to_owned(),
        ),
        scenario_valuation: None,
    }
}

fn sample_risk_report(level: RiskLevel, assessment: &str, flags_violation: bool) -> RiskReport {
    RiskReport {
        risk_level: level,
        assessment: assessment.to_owned(),
        recommended_adjustments: vec!["tighten monitoring around support".to_owned()],
        flags_violation,
    }
}
