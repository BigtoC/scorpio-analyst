use crate::{
    agents::{
        analyst::equity::{
            build_fundamental_system_prompt, build_news_system_prompt,
            build_sentiment_system_prompt, build_technical_system_prompt,
        },
        fund_manager::build_prompt_context as build_fund_manager_prompt_context,
        researcher::{
            BEARISH_SYSTEM_PROMPT, BULLISH_SYSTEM_PROMPT, MODERATOR_SYSTEM_PROMPT,
            render_researcher_system_prompt,
        },
        risk::{
            AGGRESSIVE_SYSTEM_PROMPT, CONSERVATIVE_SYSTEM_PROMPT, DualRiskStatus,
            NEUTRAL_SYSTEM_PROMPT, RISK_MODERATOR_SYSTEM_PROMPT, render_risk_system_prompt,
        },
        trader::build_prompt_context_for_test as build_trader_prompt_context,
    },
    analysis_packs::resolve_runtime_policy,
    state::{
        DataCoverageReport, DebateMessage, FundamentalData, MarketVolatilityData, NewsData,
        ProvenanceSummary, RiskLevel, RiskReport, SentimentData, TechnicalData, TradeAction,
        TradeProposal, TradingState, VixRegime, VixTrend,
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

#[must_use]
pub fn render_baseline_prompt_for_role(role: Role, scenario: PromptRenderScenario) -> String {
    let state = build_state(scenario);
    match role {
        Role::FundamentalAnalyst => build_fundamental_system_prompt(
            &state.asset_symbol,
            &state.target_date,
            state.analysis_runtime_policy.as_ref(),
        ),
        Role::SentimentAnalyst => build_sentiment_system_prompt(
            &state.asset_symbol,
            &state.target_date,
            state.analysis_runtime_policy.as_ref(),
        ),
        Role::NewsAnalyst => build_news_system_prompt(
            &state.asset_symbol,
            &state.target_date,
            state.analysis_runtime_policy.as_ref(),
        ),
        Role::TechnicalAnalyst => build_technical_system_prompt(
            &state.asset_symbol,
            &state.target_date,
            state.analysis_runtime_policy.as_ref(),
        ),
        Role::BullishResearcher => {
            render_researcher_system_prompt(BULLISH_SYSTEM_PROMPT, &state, |bundle| {
                bundle.bullish_researcher.as_ref()
            })
        }
        Role::BearishResearcher => {
            render_researcher_system_prompt(BEARISH_SYSTEM_PROMPT, &state, |bundle| {
                bundle.bearish_researcher.as_ref()
            })
        }
        Role::DebateModerator => {
            render_researcher_system_prompt(MODERATOR_SYSTEM_PROMPT, &state, |bundle| {
                bundle.debate_moderator.as_ref()
            })
        }
        Role::Trader => {
            build_trader_prompt_context(&state, &state.asset_symbol, &state.target_date)
                .system_prompt
        }
        Role::AggressiveRisk => {
            render_risk_system_prompt(AGGRESSIVE_SYSTEM_PROMPT, &state, |bundle| {
                bundle.aggressive_risk.as_ref()
            })
        }
        Role::ConservativeRisk => {
            render_risk_system_prompt(CONSERVATIVE_SYSTEM_PROMPT, &state, |bundle| {
                bundle.conservative_risk.as_ref()
            })
        }
        Role::NeutralRisk => render_risk_system_prompt(NEUTRAL_SYSTEM_PROMPT, &state, |bundle| {
            bundle.neutral_risk.as_ref()
        }),
        Role::RiskModerator => {
            render_risk_system_prompt(RISK_MODERATOR_SYSTEM_PROMPT, &state, |bundle| {
                bundle.risk_moderator.as_ref()
            })
        }
        Role::FundManager => {
            let dual_risk = DualRiskStatus::from_reports(
                state.conservative_risk_report.as_ref(),
                state.neutral_risk_report.as_ref(),
            );
            let (system_prompt, _) = build_fund_manager_prompt_context(
                &state,
                &state.asset_symbol,
                &state.target_date,
                dual_risk,
            );
            system_prompt
        }
    }
}

#[must_use]
pub fn canonical_fixture_identity() -> (&'static str, &'static str) {
    (FIXTURE_TICKER, FIXTURE_DATE)
}

fn build_state(scenario: PromptRenderScenario) -> TradingState {
    let mut state = TradingState::new(FIXTURE_TICKER, FIXTURE_DATE);
    state.analysis_runtime_policy =
        Some(resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve"));
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
    state.provenance_summary = Some(ProvenanceSummary {
        providers_used: vec![
            "finnhub".to_owned(),
            "fred".to_owned(),
            "yfinance".to_owned(),
        ],
    });
    state.set_market_volatility(sample_market_volatility());

    match scenario {
        PromptRenderScenario::AllInputsPresent => populate_all_inputs_present(&mut state),
        PromptRenderScenario::ZeroDebate => populate_zero_debate(&mut state),
        PromptRenderScenario::ZeroRisk => populate_zero_risk(&mut state),
        PromptRenderScenario::MissingAnalystData => populate_missing_analyst_data(&mut state),
    }

    state
}

fn populate_all_inputs_present(state: &mut TradingState) {
    state.set_fundamental_metrics(sample_fundamental_data());
    state.set_market_sentiment(sample_sentiment_data());
    state.set_macro_news(sample_news_data());
    state.set_technical_indicators(sample_technical_data());
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
