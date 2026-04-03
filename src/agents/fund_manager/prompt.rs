use crate::{
    agents::shared::{
        UNTRUSTED_CONTEXT_NOTICE, sanitize_date_for_prompt, sanitize_prompt_context,
        sanitize_symbol_for_prompt, serialize_prompt_value,
    },
    constants::{MAX_PROMPT_CONTEXT_CHARS, MAX_USER_PROMPT_CHARS},
    state::{DebateMessage, RiskReport, TradingState},
};

use super::validation::{state_has_missing_analyst_inputs, state_has_missing_risk_reports};
const MISSING_RISK_REPORT_NOTE: &str = "(no risk report available — treat as unknown)";
const MISSING_RISK_DISCUSSION_NOTE: &str = "(no risk discussion history available)";
const MISSING_ANALYST_DATA_NOTE: &str =
    "(data unavailable — acknowledge the gap and calibrate confidence conservatively)";

/// System prompt for the Fund Manager, from `docs/prompts.md` section 5.
pub(super) const FUND_MANAGER_SYSTEM_PROMPT: &str = "\
You are the Fund Manager for {ticker} as of {current_date}.
Your role is to make the final approve-or-reject execution decision after reviewing the trader \
proposal and all risk inputs.

{untrusted_context_notice}

Available inputs:
- Trader proposal: {trader_proposal}
- Aggressive risk report: {aggressive_risk_report}
- Neutral risk report: {neutral_risk_report}
- Conservative risk report: {conservative_risk_report}
- Risk discussion summary: {risk_discussion_history}
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Past learnings: {past_memory_str}

Current market price: {current_price}

Return ONLY a JSON object matching `ExecutionStatus`:
- `decision`: `Approved` or `Rejected`
- `action`: one of `Buy`, `Sell`, `Hold`
- `rationale`: concise audit-ready explanation
- `decided_at`: use `{current_date}` unless the runtime provides a more precise timestamp
- `entry_guidance`: (required when action is Hold or Sell) a specific tactical entry condition, \
e.g. \"tactical BUY on any dip below $570-$575\" or \"accumulate below $145 on weakness\". \
Reference concrete price levels derived from support/resistance, valuation floor, or technical signals.
- `suggested_position`: recommended portfolio allocation with scaling guidance, \
e.g. \"5-12% of portfolio (add 2-4% on weakness) - maintain conservative sizing while volatility premium persists\". \
Calibrate size to conviction level, volatility, and risk tolerance.

Instructions:
1. Review the trader proposal and all risk inputs carefully.
2. Apply the deterministic safety rule: if BOTH the Conservative and Neutral risk reports clearly \
flag a material violation (`flags_violation == true`), reject the proposal.
3. Otherwise, make an evidence-based decision using the full input set.
4. Ground the decision in the trader's valuation assessment - use it to anchor price levels \
in `entry_guidance` and calibrate `suggested_position`.
5. Approve only if the proposal's action, target, stop, and confidence are defensible.
6. If rejecting, make the blocking reason explicit in `rationale`.
7. If any risk report or analyst input is missing, acknowledge the gap in `rationale` and \
calibrate confidence conservatively.
8. If the final `action` is Hold or Sell, you MUST provide `entry_guidance` with a specific \
price level or condition at which the asset becomes a buy.
9. Always provide `suggested_position` with concrete portfolio percentage ranges.
10. Return ONLY the single JSON object required by `ExecutionStatus`.
11. Set `action` to the trade direction you endorse. This may match the trader's proposed \
action or differ if your review warrants a change. If your decision is `Rejected`, \
`Hold` is the expected default unless the rejection is specifically about direction \
(e.g., the trader said Buy but evidence supports Sell).

Do not restate the entire pipeline.";

pub(super) fn build_prompt_context(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
) -> (String, String) {
    let symbol = sanitize_symbol_for_prompt(symbol);
    let target_date = sanitize_date_for_prompt(target_date);

    let missing_analyst_data = state_has_missing_analyst_inputs(state);
    let missing_risk_reports = state_has_missing_risk_reports(state);

    let data_quality_note = if missing_analyst_data || missing_risk_reports {
        "One or more upstream inputs are missing. Explicitly acknowledge the missing data in \
         `rationale` and lower confidence appropriately."
    } else {
        "All upstream inputs are available for this run."
    };

    let system_prompt = FUND_MANAGER_SYSTEM_PROMPT
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date)
        .replace("{trader_proposal}", "see user context")
        .replace("{aggressive_risk_report}", "see user context")
        .replace("{neutral_risk_report}", "see user context")
        .replace("{conservative_risk_report}", "see user context")
        .replace("{risk_discussion_history}", "see user context")
        .replace("{fundamental_report}", "see user context")
        .replace("{technical_report}", "see user context")
        .replace("{sentiment_report}", "see user context")
        .replace("{news_report}", "see user context")
        .replace("{past_memory_str}", "")
        .replace("{untrusted_context_notice}", UNTRUSTED_CONTEXT_NOTICE)
        .replace(
            "{current_price}",
            &state
                .current_price
                .map_or_else(|| "unavailable".to_owned(), |p| format!("{p:.2}")),
        );

    let user_prompt = build_user_prompt(state, &symbol, &target_date, data_quality_note);

    (system_prompt, user_prompt)
}

fn serialize_risk_discussion_history(history: &[DebateMessage]) -> String {
    if history.is_empty() {
        return sanitize_prompt_context(MISSING_RISK_DISCUSSION_NOTE);
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
    match report {
        Some(risk_report) => serialize_prompt_value(&Some(risk_report)),
        None => sanitize_prompt_context(MISSING_RISK_REPORT_NOTE),
    }
}

fn build_user_prompt(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
    data_quality_note: &str,
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
        &format!("Data quality note: {}", data_quality_note),
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
            serialize_optional_value_with_missing_note(
                &state.fundamental_metrics,
                MISSING_ANALYST_DATA_NOTE,
            )
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Technical data: {}",
            serialize_optional_value_with_missing_note(
                &state.technical_indicators,
                MISSING_ANALYST_DATA_NOTE,
            )
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "Sentiment data: {}",
            serialize_optional_value_with_missing_note(
                &state.market_sentiment,
                MISSING_ANALYST_DATA_NOTE,
            )
        ),
        MAX_USER_PROMPT_CHARS,
    );
    push_bounded_line(
        &mut prompt,
        &format!(
            "News data: {}",
            serialize_optional_value_with_missing_note(
                &state.macro_news,
                MISSING_ANALYST_DATA_NOTE,
            )
        ),
        MAX_USER_PROMPT_CHARS,
    );

    prompt
}

fn serialize_optional_value_with_missing_note<T: serde::Serialize>(
    value: &Option<T>,
    missing_note: &str,
) -> String {
    match value {
        Some(_) => serialize_prompt_value(value),
        None => sanitize_prompt_context(missing_note),
    }
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
    use super::{FUND_MANAGER_SYSTEM_PROMPT, build_prompt_context};
    use crate::state::{
        DebateMessage, FundamentalData, ImpactDirection, MacroEvent, NewsArticle, NewsData,
        RiskLevel, RiskReport, SentimentData, SentimentSource, TechnicalData, TradeAction,
        TradeProposal, TradingState,
    };

    fn valid_proposal() -> TradeProposal {
        TradeProposal {
            action: TradeAction::Buy,
            target_price: 185.50,
            stop_loss: 178.00,
            confidence: 0.82,
            rationale: "Strong fundamentals and momentum support this Buy.".to_owned(),
            valuation_assessment: None,
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
        state.trader_proposal = Some(valid_proposal());
        state.aggressive_risk_report = Some(no_violation_risk_report(RiskLevel::Aggressive));
        state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));
        state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));
        state.fundamental_metrics = Some(FundamentalData {
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
        state.technical_indicators = Some(TechnicalData {
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
        state.market_sentiment = Some(SentimentData {
            overall_score: 0.34,
            source_breakdown: vec![SentimentSource {
                source_name: "news".to_owned(),
                score: 0.34,
                sample_size: 12,
            }],
            engagement_peaks: Vec::new(),
            summary: "Modestly positive.".to_owned(),
        });
        state.macro_news = Some(NewsData {
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
    fn system_prompt_contains_safety_net_instructions() {
        assert!(
            FUND_MANAGER_SYSTEM_PROMPT.contains("flags_violation"),
            "system prompt must mention flags_violation"
        );
        assert!(
            FUND_MANAGER_SYSTEM_PROMPT.contains("Approved"),
            "system prompt must mention Approved decision"
        );
        assert!(
            FUND_MANAGER_SYSTEM_PROMPT.contains("Rejected"),
            "system prompt must mention Rejected decision"
        );
        assert!(
            FUND_MANAGER_SYSTEM_PROMPT.contains("action"),
            "system prompt must mention action field"
        );
    }

    #[test]
    fn prompt_context_includes_serialized_trader_proposal_and_risk_reports() {
        let state = populated_state();
        let (_system_prompt, user_prompt) =
            build_prompt_context(&state, &state.asset_symbol, &state.target_date);
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
        state.trader_proposal = Some(valid_proposal());
        let (_system_prompt, user_prompt) =
            build_prompt_context(&state, &state.asset_symbol, &state.target_date);
        assert!(
            user_prompt.contains("no risk report available"),
            "prompt should note missing risk reports"
        );
    }

    #[test]
    fn untrusted_serialized_context_is_not_embedded_in_system_prompt() {
        let state = populated_state();
        let (system_prompt, user_prompt) =
            build_prompt_context(&state, &state.asset_symbol, &state.target_date);
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
        let (_system_prompt, user_prompt) =
            build_prompt_context(&state, &state.asset_symbol, &state.target_date);
        assert!(
            user_prompt.contains("Conservative and Neutral disagree"),
            "prompt should include risk discussion history"
        );
    }
}
