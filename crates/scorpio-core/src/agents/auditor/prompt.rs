use crate::{
    agents::shared::{sanitize_date_for_prompt, sanitize_symbol_for_prompt},
    error::TradingError,
    state::TradingState,
};

pub(crate) fn build_system_prompt(state: &TradingState) -> Result<String, TradingError> {
    let policy = state.analysis_runtime_policy.as_ref().ok_or_else(|| {
        TradingError::Config(anyhow::anyhow!(
            "auditor prompt: missing runtime policy — preflight must run before auditor"
        ))
    })?;
    if policy.prompt_bundle.auditor.is_empty() {
        return Err(TradingError::Config(anyhow::anyhow!(
            "auditor prompt: auditor slot is empty — pack must supply a non-empty auditor prompt \
             when auditor_enabled = true"
        )));
    }
    let symbol = sanitize_symbol_for_prompt(&state.asset_symbol);
    let target_date = sanitize_date_for_prompt(&state.target_date);
    let rendered = policy
        .prompt_bundle
        .auditor
        .as_ref()
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date);
    Ok(crate::analysis_packs::append_leverage_warning_if_needed(
        rendered,
        etf_leverage_factor_from_state(state),
    ))
}

fn etf_leverage_factor_from_state(state: &TradingState) -> Option<f64> {
    use crate::state::ScenarioValuation;
    match state.derived_valuation()?.scenario {
        ScenarioValuation::Etf(ref etf) => etf.leverage_factor,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TradingState;
    use crate::testing::with_baseline_runtime_policy;

    #[test]
    fn build_system_prompt_fails_without_runtime_policy() {
        let state = TradingState::new("AAPL".to_owned(), "2026-05-10".to_owned());
        assert!(build_system_prompt(&state).is_err());
    }

    #[test]
    fn build_system_prompt_fails_with_empty_auditor_slot() {
        let mut state = TradingState::new("AAPL".to_owned(), "2026-05-10".to_owned());
        with_baseline_runtime_policy(&mut state);
        if let Some(ref mut policy) = state.analysis_runtime_policy {
            policy.prompt_bundle.auditor = std::borrow::Cow::Borrowed("");
        }
        assert!(build_system_prompt(&state).is_err());
    }

    #[test]
    fn build_system_prompt_substitutes_ticker() {
        let mut state = TradingState::new("MSFT".to_owned(), "2026-05-10".to_owned());
        with_baseline_runtime_policy(&mut state);
        let prompt = build_system_prompt(&state).expect("prompt should build");
        assert!(
            !prompt.contains("{ticker}"),
            "placeholder must be substituted"
        );
        assert!(
            prompt.contains("MSFT"),
            "ticker must appear in rendered prompt"
        );
    }

    #[test]
    fn auditor_prompt_carries_leverage_warning_for_levered_etf() {
        use crate::state::{
            AssetShape, DerivedValuation, EtfDataAvailability, EtfValuation, PremiumBand,
            PremiumSnapshot, ScenarioValuation,
        };

        let mut state = TradingState::new("TQQQ".to_owned(), "2026-05-27".to_owned());
        let manifest =
            crate::analysis_packs::resolve_pack(crate::analysis_packs::PackId::EtfBaseline);
        let policy = crate::analysis_packs::resolve_runtime_policy_for_manifest(&manifest)
            .expect("etf_baseline manifest must resolve to a valid runtime policy");
        state.analysis_runtime_policy = Some(policy);
        state.set_derived_valuation(DerivedValuation {
            asset_shape: AssetShape::Fund,
            scenario: ScenarioValuation::Etf(EtfValuation {
                premium: PremiumSnapshot {
                    nav: Some(50.0),
                    market_price: 50.0,
                    bid: None,
                    ask: None,
                    premium_pct: None,
                    category_band: PremiumBand::Unknown,
                    bid_ask_spread_pct: None,
                    as_of: chrono::Utc::now(),
                },
                composition: None,
                tracking: None,
                options_gex: None,
                category: None,
                leverage_factor: Some(-2.0),
                flags: EtfDataAvailability::default(),
            }),
        });

        let prompt = build_system_prompt(&state).expect("prompt build");
        assert!(
            prompt.contains("Daily-reset products"),
            "auditor prompt must include the leverage warning body: {prompt}"
        );
        assert!(
            prompt.contains("-2x"),
            "auditor prompt must substitute the factor: {prompt}"
        );
    }
}
