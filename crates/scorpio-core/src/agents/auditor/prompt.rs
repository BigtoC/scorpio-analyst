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
    Ok(policy
        .prompt_bundle
        .auditor
        .as_ref()
        .replace("{ticker}", &symbol)
        .replace("{current_date}", &target_date))
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
}
