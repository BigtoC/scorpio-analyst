use crate::{
    agents::shared::{
        UNTRUSTED_CONTEXT_NOTICE, analysis_emphasis_for_prompt, build_data_quality_context,
        build_enrichment_context, build_evidence_context, build_pack_context,
        build_thesis_memory_context, build_valuation_context, sanitize_date_for_prompt,
        sanitize_prompt_context, sanitize_symbol_for_prompt, serialize_prompt_value,
    },
    state::TradingState,
};

pub(super) const MISSING_CONSENSUS_NOTE: &str =
    "(no debate consensus available - base the proposal on analyst data alone)";

#[cfg_attr(
    any(test, feature = "test-helpers"),
    derive(Debug, Clone, PartialEq, Eq)
)]
pub(crate) struct PromptContext {
    pub(crate) system_prompt: String,
    pub(crate) user_prompt: String,
}

fn trader_system_prompt_template(state: &TradingState) -> &str {
    state
        .analysis_runtime_policy
        .as_ref()
        .expect(
            "trader prompt: missing runtime policy — preflight is the sole writer of \
             state.analysis_runtime_policy; tests bypassing preflight must use \
             `with_baseline_runtime_policy`",
        )
        .prompt_bundle
        .trader
        .as_ref()
}

pub(crate) fn build_prompt_context(
    state: &TradingState,
    symbol: &str,
    target_date: &str,
) -> PromptContext {
    let symbol = sanitize_symbol_for_prompt(symbol);
    let target_date = sanitize_date_for_prompt(target_date);
    let missing_analyst_data = state.fundamental_metrics().is_none()
        || state.technical_indicators().is_none()
        || state.market_sentiment().is_none()
        || state.macro_news().is_none();
    let missing_consensus = state.consensus_summary.is_none();

    let data_quality_note = if missing_analyst_data || missing_consensus {
        "One or more upstream inputs are missing. Explicitly acknowledge the missing data in `rationale` and lower confidence appropriately."
    } else {
        "All analyst inputs and the debate consensus are available for this run."
    };

    let system_prompt = trader_system_prompt_template(state)
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date)
        .replace("{analysis_emphasis}", &analysis_emphasis_for_prompt(state))
        .replace(
            "{consensus_summary}",
            &sanitize_prompt_context(
                state
                    .consensus_summary
                    .as_deref()
                    .unwrap_or(MISSING_CONSENSUS_NOTE),
            ),
        )
        .replace(
            "{fundamental_report}",
            &serialize_prompt_value(&state.fundamental_metrics()),
        )
        .replace(
            "{technical_report}",
            &serialize_prompt_value(&state.technical_indicators()),
        )
        .replace(
            "{sentiment_report}",
            &serialize_prompt_value(&state.market_sentiment()),
        )
        .replace(
            "{news_report}",
            &serialize_prompt_value(&state.macro_news()),
        )
        .replace(
            "{market_volatility_report}",
            &serialize_prompt_value(&state.market_volatility()),
        )
        .replace("{past_memory_str}", "see user context")
        .replace("{data_quality_note}", data_quality_note)
        .replace("{untrusted_context_notice}", UNTRUSTED_CONTEXT_NOTICE);

    let enrichment = build_enrichment_context(state);
    let enrichment_section = if enrichment.is_empty() {
        String::new()
    } else {
        format!("\n\n{enrichment}")
    };
    let pack = build_pack_context(state);
    let pack_section = if pack.is_empty() {
        String::new()
    } else {
        format!("\n\n{pack}")
    };

    let user_prompt = format!(
        "Produce a TradeProposal JSON for {} as of {}.\n\nPast learnings: {}\n\n{}\n\n{}\n\n{}{}{}",
        symbol,
        target_date,
        build_thesis_memory_context(state),
        build_valuation_context(state),
        build_evidence_context(state),
        build_data_quality_context(state),
        enrichment_section,
        pack_section,
    );

    PromptContext {
        system_prompt,
        user_prompt,
    }
}
