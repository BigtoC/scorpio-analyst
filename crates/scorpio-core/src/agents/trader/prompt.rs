use crate::{
    agents::shared::{
        UNTRUSTED_CONTEXT_NOTICE, analysis_emphasis_for_prompt, build_data_quality_context,
        build_enrichment_context, build_evidence_context, build_pack_context,
        build_thesis_memory_context, build_valuation_context, compact_technical_report,
        sanitize_date_for_prompt, sanitize_prompt_context, sanitize_symbol_for_prompt,
        serialize_prompt_value,
    },
    state::TradingState,
};

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

    let system_prompt = trader_system_prompt_template(state)
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date)
        .replace("{analysis_emphasis}", &analysis_emphasis_for_prompt(state))
        .replace(
            "{consensus_summary}",
            &state
                .consensus_summary
                .as_deref()
                .map(sanitize_prompt_context)
                .unwrap_or_else(|| "null".to_owned()),
        )
        .replace(
            "{fundamental_report}",
            &serialize_prompt_value(&state.fundamental_metrics()),
        )
        .replace(
            "{technical_report}",
            &state
                .technical_indicators()
                .map(compact_technical_report)
                .unwrap_or_else(|| "null".to_owned()),
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
        .replace("{data_quality_note}", "see user context")
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        data::traits::options::{IvTermPoint, NearTermStrike, OptionsOutcome, OptionsSnapshot},
        state::{TechnicalData, TechnicalOptionsContext, TradingState},
        testing::with_baseline_runtime_policy,
    };

    fn sample_technical_with_options_context() -> TechnicalData {
        let snap = OptionsSnapshot {
            spot_price: 182.0,
            atm_iv: 0.28,
            iv_term_structure: vec![
                IvTermPoint {
                    expiration: "2026-01-17".to_owned(),
                    atm_iv: 0.28,
                },
                IvTermPoint {
                    expiration: "2026-02-21".to_owned(),
                    atm_iv: 0.31,
                },
            ],
            put_call_volume_ratio: 1.1,
            put_call_oi_ratio: 1.0,
            max_pain_strike: 180.0,
            near_term_expiration: "2026-01-17".to_owned(),
            near_term_strikes: vec![
                NearTermStrike {
                    strike: 175.0,
                    call_iv: Some(0.25),
                    put_iv: Some(0.30),
                    call_volume: Some(1_000),
                    put_volume: Some(2_000),
                    call_oi: Some(5_000),
                    put_oi: Some(7_500),
                },
                NearTermStrike {
                    strike: 180.0,
                    call_iv: Some(0.27),
                    put_iv: Some(0.28),
                    call_volume: Some(3_000),
                    put_volume: Some(1_500),
                    call_oi: Some(8_000),
                    put_oi: Some(4_500),
                },
            ],
        };

        TechnicalData {
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
            options_summary: Some("Near-term IV elevated.".to_owned()),
            options_context: Some(TechnicalOptionsContext::Available {
                outcome: OptionsOutcome::Snapshot(snap),
            }),
        }
    }

    #[test]
    fn trader_prompt_context_projects_options_context() {
        let mut state = TradingState::new("AAPL", "2026-01-17");
        with_baseline_runtime_policy(&mut state);
        state.set_technical_indicators(sample_technical_with_options_context());

        let ctx = build_prompt_context(&state, "AAPL", "2026-01-17");
        // The technical_report is injected into the system prompt for the trader
        let system = &ctx.system_prompt;

        // 1. options_context key must appear in the system prompt
        assert!(
            system.contains("options_context"),
            "options_context must appear in trader system prompt: {system}"
        );
        // 2. Compact summary fields must be present
        assert!(system.contains("atm_iv"), "atm_iv missing: {system}");
        assert!(
            system.contains("put_call_volume_ratio"),
            "put_call_volume_ratio missing: {system}"
        );
        assert!(
            system.contains("max_pain_strike"),
            "max_pain_strike missing: {system}"
        );
        assert!(
            system.contains("near_term_expiration"),
            "near_term_expiration missing: {system}"
        );
        // 3. Raw near_term_strikes array must NOT appear verbatim
        assert!(
            !system.contains("near_term_strikes"),
            "near_term_strikes array must be stripped from trader system prompt: {system}"
        );
        // 4. iv_term_structure array must NOT appear
        assert!(
            !system.contains("iv_term_structure"),
            "iv_term_structure array must be stripped from trader system prompt: {system}"
        );
    }

    #[test]
    fn trader_prompt_context_sanitizes_adversarial_options_summary() {
        // options_summary is model-produced text — it must be sanitized before
        // reaching the system prompt. Secret-pattern adversarial content (e.g. a
        // bearer token leak) must be redacted.
        let mut state = TradingState::new("AAPL", "2026-01-17");
        with_baseline_runtime_policy(&mut state);
        let mut data = sample_technical_with_options_context();
        data.options_summary =
            Some("Normal IV. Authorization: Bearer sk-ant-secret99 injected.".to_owned());
        state.set_technical_indicators(data);

        let ctx = build_prompt_context(&state, "AAPL", "2026-01-17");
        let system = &ctx.system_prompt;

        // The secret-like token must be redacted, not echoed verbatim
        assert!(
            !system.contains("sk-ant-secret99"),
            "adversarial bearer token in options_summary must be redacted: {system}"
        );
        assert!(
            system.contains("[REDACTED]"),
            "redaction marker must appear in output: {system}"
        );
    }

    #[test]
    fn trader_prompt_context_handles_legacy_options_summary_blob() {
        let mut state = TradingState::new("AAPL", "2026-01-17");
        with_baseline_runtime_policy(&mut state);
        state.set_technical_indicators(TechnicalData {
            rsi: Some(55.0),
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
            summary: "Legacy run.".to_owned(),
            options_summary: Some("{ old raw json blob }".to_owned()),
            options_context: None,
        });

        let ctx = build_prompt_context(&state, "AAPL", "2026-01-17");
        let system = &ctx.system_prompt;

        // Legacy blob passes through as a plain string field
        assert!(
            system.contains("old raw json blob"),
            "legacy options_summary must pass through: {system}"
        );
        // No "options_context" JSON key in the serialized technical blob since it's None
        assert!(
            !system.contains(r#""options_context""#),
            "options_context JSON key must be absent from compact report for legacy data: {system}"
        );
    }

    #[test]
    fn trader_prompt_context_handles_fetch_failed_options_context() {
        let mut state = TradingState::new("AAPL", "2026-01-17");
        with_baseline_runtime_policy(&mut state);
        state.set_technical_indicators(TechnicalData {
            rsi: Some(55.0),
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
            summary: "OK".to_owned(),
            options_summary: None,
            options_context: Some(TechnicalOptionsContext::FetchFailed {
                reason: "network timeout".to_owned(),
            }),
        });

        let ctx = build_prompt_context(&state, "AAPL", "2026-01-17");
        let system = &ctx.system_prompt;

        assert!(
            system.contains("fetch_failed"),
            "fetch_failed status must appear in trader system prompt: {system}"
        );
        assert!(
            system.contains("options_context"),
            "options_context must appear for FetchFailed: {system}"
        );
    }
}
