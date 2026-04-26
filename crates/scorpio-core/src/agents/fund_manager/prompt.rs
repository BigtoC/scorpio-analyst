use crate::{
    agents::{
        risk::DualRiskStatus,
        shared::{
            UNTRUSTED_CONTEXT_NOTICE, analysis_emphasis_for_prompt, build_data_quality_context,
            build_enrichment_context, build_evidence_context, build_pack_context,
            build_thesis_memory_context, build_valuation_context, sanitize_date_for_prompt,
            sanitize_prompt_context, sanitize_symbol_for_prompt, serialize_prompt_value,
        },
    },
    constants::{MAX_PROMPT_CONTEXT_CHARS, MAX_USER_PROMPT_CHARS},
    state::{DebateMessage, RiskReport, TradingState},
};

use super::validation::state_has_missing_analyst_inputs;

fn fund_manager_system_prompt_template(state: &TradingState) -> &str {
    state
        .analysis_runtime_policy
        .as_ref()
        .expect(
            "fund manager prompt: missing runtime policy — preflight is the sole writer of \
             state.analysis_runtime_policy; tests bypassing preflight must use \
             `with_baseline_runtime_policy`",
        )
        .prompt_bundle
        .fund_manager
        .as_ref()
}

pub(crate) fn build_prompt_context(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
    dual_risk_status: DualRiskStatus,
) -> (String, String) {
    let symbol = sanitize_symbol_for_prompt(symbol);
    let target_date = sanitize_date_for_prompt(target_date);

    let missing_analyst_data = state_has_missing_analyst_inputs(state);
    let missing_risk_reports = dual_risk_status != DualRiskStatus::StageDisabled
        && (state.aggressive_risk_report.is_none()
            || state.neutral_risk_report.is_none()
            || state.conservative_risk_report.is_none());
    let upstream_data_state = if missing_analyst_data || missing_risk_reports {
        "incomplete"
    } else {
        "complete"
    };

    let system_prompt = fund_manager_system_prompt_template(state)
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date)
        .replace("{analysis_emphasis}", &analysis_emphasis_for_prompt(state))
        .replace("{trader_proposal}", "see user context")
        .replace("{aggressive_risk_report}", "see user context")
        .replace("{neutral_risk_report}", "see user context")
        .replace("{conservative_risk_report}", "see user context")
        .replace("{risk_discussion_history}", "see user context")
        .replace("{fundamental_report}", "see user context")
        .replace("{technical_report}", "see user context")
        .replace("{sentiment_report}", "see user context")
        .replace("{news_report}", "see user context")
        .replace("{past_memory_str}", "see user context")
        .replace("{untrusted_context_notice}", UNTRUSTED_CONTEXT_NOTICE)
        .replace(
            "{current_price}",
            &state
                .current_price
                .map_or_else(|| "unavailable".to_owned(), |p| format!("{p:.2}")),
        );

    let user_prompt = build_user_prompt(
        state,
        &symbol,
        &target_date,
        upstream_data_state,
        dual_risk_status,
    );

    (system_prompt, user_prompt)
}

fn serialize_risk_discussion_history(history: &[DebateMessage]) -> String {
    if history.is_empty() {
        return "null".to_owned();
    }

    let mut joined = String::new();
    for message in history {
        let line = sanitize_prompt_context(&format!("[{}]: {}", message.role, message.content));
        push_bounded_line(&mut joined, &line, MAX_PROMPT_CONTEXT_CHARS);
        if joined.chars().count() >= MAX_PROMPT_CONTEXT_CHARS {
            break;
        }
    }

    joined
}

fn serialize_optional_risk_report(report: &Option<RiskReport>) -> String {
    serialize_prompt_value(report)
}

fn build_user_prompt(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
    upstream_data_state: &str,
    dual_risk_status: DualRiskStatus,
) -> String {
    let mut prompt = String::new();

    push_bounded_line(
        &mut prompt,
        &format!(
            "Produce an ExecutionStatus JSON for {} as of {}.",
            symbol, target_date
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Dual-risk escalation: {}",
            dual_risk_status.as_prompt_value()
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!("Upstream data state: {upstream_data_state}"),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!("Past learnings: {}", build_thesis_memory_context(state)),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &build_valuation_context(state),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Trader proposal: {}",
            serialize_prompt_value(&state.trader_proposal)
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Aggressive risk report: {}",
            serialize_optional_risk_report(&state.aggressive_risk_report)
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Neutral risk report: {}",
            serialize_optional_risk_report(&state.neutral_risk_report)
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Conservative risk report: {}",
            serialize_optional_risk_report(&state.conservative_risk_report)
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Risk discussion history: {}",
            serialize_risk_discussion_history(&state.risk_discussion_history)
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Fundamental data: {}",
            serialize_prompt_value(&state.fundamental_metrics())
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Technical data: {}",
            serialize_prompt_value(&state.technical_indicators())
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Sentiment data: {}",
            serialize_prompt_value(&state.market_sentiment())
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!("News data: {}", serialize_prompt_value(&state.macro_news())),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &build_evidence_context(state),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &build_data_quality_context(state),
        MAX_USER_PROMPT_CHARS,
    );
    let pack_context = build_pack_context(state);
    if !pack_context.is_empty() {
        push_bounded_line(&mut prompt, &pack_context, MAX_USER_PROMPT_CHARS);
    }
    push_bounded_line(
        &mut prompt,
        &build_enrichment_context(state),
        MAX_USER_PROMPT_CHARS,
    );

    prompt
}
#[cfg(test)]
pub(super) fn build_user_prompt_for_test(dual_risk_status: DualRiskStatus) -> String {
    use crate::state::TradingState;
    let state = TradingState::new("AAPL", "2026-01-15");
    build_user_prompt(
        &state,
        "AAPL",
        "2026-01-15",
        "test_upstream_data_state",
        dual_risk_status,
    )
}

fn push_bounded_line(buffer: &mut String, line: &str, max_chars: usize) {
    if buffer.chars().count() >= max_chars {
        return;
    }

    let needs_newline = !buffer.is_empty();
    let reserved_for_newline = usize::from(needs_newline);
    let available = max_chars
        .saturating_sub(buffer.chars().count())
        .saturating_sub(reserved_for_newline);

    if available == 0 {
        return;
    }

    let truncated_line: String = line.chars().take(available).collect();
    if truncated_line.is_empty() {
        return;
    }

    if needs_newline {
        buffer.push('\n');
    }
    buffer.push_str(&truncated_line);
}

#[cfg(test)]
mod tests {
    use super::build_prompt_context;
    use crate::{
        agents::risk::DualRiskStatus,
        analysis_packs::resolve_runtime_policy,
        state::{
            DebateMessage, FundamentalData, ImpactDirection, MacroEvent, NewsArticle, NewsData,
            RiskLevel, RiskReport, SentimentData, SentimentSource, TechnicalData, TradeAction,
            TradeProposal, TradingState,
        },
        workflow::Role,
    };

    fn baseline_fund_manager_prompt() -> &'static str {
        crate::testing::baseline_pack_prompt_for_role(Role::FundManager)
    }

    fn with_baseline_policy(state: &mut TradingState) {
        state.analysis_runtime_policy = Some(
            resolve_runtime_policy("baseline").expect("baseline runtime policy should resolve"),
        );
    }

    fn valid_proposal() -> TradeProposal {
        TradeProposal {
            action: TradeAction::Buy,
            target_price: 185.50,
            stop_loss: 178.00,
            confidence: 0.82,
            rationale: "Strong fundamentals and momentum support this Buy.".to_owned(),
            valuation_assessment: None,
            scenario_valuation: None,
        }
    }

    fn no_violation_risk_report(level: RiskLevel) -> RiskReport {
        RiskReport {
            risk_level: level,
            assessment: "Risk is within acceptable bounds.".to_owned(),
            recommended_adjustments: vec![],
            flags_violation: false,
        }
    }

    fn populated_state() -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-03-15");
        with_baseline_policy(&mut state);
        state.trader_proposal = Some(valid_proposal());
        state.aggressive_risk_report = Some(no_violation_risk_report(RiskLevel::Aggressive));
        state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));
        state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));
        state.set_fundamental_metrics(FundamentalData {
            revenue_growth_pct: Some(0.12),
            pe_ratio: Some(28.5),
            eps: Some(6.1),
            current_ratio: Some(1.3),
            debt_to_equity: Some(0.8),
            gross_margin: Some(0.43),
            net_income: Some(9.5e10),
            insider_transactions: Vec::new(),
            summary: "Strong margins.".to_owned(),
        });
        state.set_technical_indicators(TechnicalData {
            rsi: Some(58.0),
            macd: None,
            atr: Some(3.1),
            sma_20: Some(182.0),
            sma_50: Some(176.0),
            ema_12: Some(183.0),
            ema_26: Some(178.0),
            bollinger_upper: Some(188.0),
            bollinger_lower: Some(172.0),
            support_level: Some(176.5),
            resistance_level: Some(187.5),
            volume_avg: Some(65_000_000.0),
            summary: "Momentum constructive.".to_owned(),
        });
        state.set_market_sentiment(SentimentData {
            overall_score: 0.34,
            source_breakdown: vec![SentimentSource {
                source_name: "news".to_owned(),
                score: 0.34,
                sample_size: 12,
            }],
            engagement_peaks: Vec::new(),
            summary: "Modestly positive.".to_owned(),
        });
        state.set_macro_news(NewsData {
            articles: vec![NewsArticle {
                title: "Apple outlook improves".to_owned(),
                source: "Reuters".to_owned(),
                published_at: "2026-03-14T12:00:00Z".to_owned(),
                relevance_score: Some(0.9),
                snippet: "Demand resilience offsets macro concerns.".to_owned(),
            }],
            macro_events: vec![MacroEvent {
                event: "Fed holds rates".to_owned(),
                impact_direction: ImpactDirection::Neutral,
                confidence: 0.7,
            }],
            summary: "Macro backdrop stable.".to_owned(),
        });
        state
    }

    #[test]
    fn system_prompt_contains_decision_instructions() {
        let prompt = baseline_fund_manager_prompt();
        assert!(
            prompt.contains("Dual-risk escalation:"),
            "system prompt must contain the dual-risk escalation indicator reference"
        );
        assert!(
            prompt.contains("Approved"),
            "system prompt must mention Approved decision"
        );
        assert!(
            prompt.contains("Rejected"),
            "system prompt must mention Rejected decision"
        );
        assert!(
            prompt.contains("action"),
            "system prompt must mention action field"
        );
    }

    #[test]
    fn prompt_context_includes_serialized_trader_proposal_and_risk_reports() {
        let state = populated_state();
        let (_system_prompt, user_prompt) = build_prompt_context(
            &state,
            &state.asset_symbol,
            &state.target_date,
            DualRiskStatus::Absent,
        );
        assert!(
            user_prompt.contains("target_price"),
            "user prompt must include serialized trader proposal"
        );
        assert!(
            user_prompt.contains("flags_violation"),
            "user prompt must include serialized risk reports"
        );
    }

    #[test]
    fn prompt_context_uses_missing_note_when_risk_reports_absent() {
        let mut state = TradingState::new("AAPL", "2026-03-15");
        with_baseline_policy(&mut state);
        state.trader_proposal = Some(valid_proposal());
        let (_system_prompt, user_prompt) = build_prompt_context(
            &state,
            &state.asset_symbol,
            &state.target_date,
            DualRiskStatus::Unknown,
        );
        assert!(
            user_prompt.contains("Aggressive risk report: null"),
            "prompt should serialize missing risk reports as null"
        );
    }

    #[test]
    fn untrusted_serialized_context_is_not_embedded_in_system_prompt() {
        let state = populated_state();
        let (system_prompt, user_prompt) = build_prompt_context(
            &state,
            &state.asset_symbol,
            &state.target_date,
            DualRiskStatus::Absent,
        );
        assert!(
            !system_prompt.contains("target_price"),
            "serialized proposal should stay out of the system prompt"
        );
        assert!(
            !system_prompt.contains("Risk is within acceptable bounds."),
            "serialized risk report content should stay out of the system prompt"
        );
        assert!(
            user_prompt.contains("target_price"),
            "serialized proposal should be placed in the user prompt"
        );
    }

    #[test]
    fn prompt_context_includes_non_empty_risk_discussion_history() {
        let mut state = populated_state();
        state.risk_discussion_history.push(DebateMessage {
            role: "moderator".to_owned(),
            content: "Conservative and Neutral disagree on stop-loss width.".to_owned(),
        });
        let (_system_prompt, user_prompt) = build_prompt_context(
            &state,
            &state.asset_symbol,
            &state.target_date,
            DualRiskStatus::Absent,
        );
        assert!(
            user_prompt.contains("Conservative and Neutral disagree"),
            "prompt should include risk discussion history"
        );
    }

    #[test]
    fn prompt_context_includes_enrichment_status_and_payload() {
        use crate::{
            data::adapters::{
                EnrichmentStatus, estimates::ConsensusEvidence, events::EventNewsEvidence,
            },
            state::EnrichmentState,
        };

        let mut state = populated_state();
        state.enrichment_event_news = EnrichmentState {
            status: EnrichmentStatus::Available,
            payload: Some(vec![EventNewsEvidence {
                symbol: "AAPL".to_owned(),
                event_timestamp: "2026-03-14T12:00:00Z".to_owned(),
                event_type: "guidance_update".to_owned(),
                headline: "Apple raises guidance".to_owned(),
                impact: Some("positive".to_owned()),
            }]),
        };
        state.enrichment_consensus = EnrichmentState {
            status: EnrichmentStatus::FetchFailed(
                "Yahoo Finance earnings trend unavailable".to_owned(),
            ),
            payload: Some(ConsensusEvidence {
                symbol: "AAPL".to_owned(),
                eps_estimate: Some(2.5),
                revenue_estimate_m: Some(95_000.0),
                analyst_count: Some(35),
                as_of_date: "2026-03-15".to_owned(),
            }),
        };

        let (_system_prompt, user_prompt) = build_prompt_context(
            &state,
            &state.asset_symbol,
            &state.target_date,
            DualRiskStatus::Absent,
        );

        assert!(user_prompt.contains("Event-news enrichment"));
        assert!(user_prompt.contains("Apple raises guidance"));
        assert!(user_prompt.contains("Consensus estimates status: fetch_failed"));
        assert!(user_prompt.contains("Yahoo Finance earnings trend unavailable"));
    }
}
