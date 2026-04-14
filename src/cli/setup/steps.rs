//! Interactive setup wizard step functions.
//!
//! Each public `stepN_*` function drives one wizard step via `inquire` prompts and
//! delegates state mutations to a pure `apply_*` / `validate_*` helper so the logic
//! can be unit-tested without touching stdin.
//!
//! **Testing note:** The `step5_health_check` function calls a real LLM via
//! `prompt_with_retry` and cannot be driven in unit tests; it is covered by manual
//! QA and the Unit 5 smoke test.

use std::time::Duration;

use anyhow::Context;
use inquire::{PasswordDisplayMode, validator::Validation};

use crate::constants::HEALTH_CHECK_TIMEOUT_SECS;
use crate::{
    cli::setup::config_file::PartialConfig,
    error::RetryPolicy,
    providers::{ModelTier, ProviderId},
};

/// Wizard-visible providers — Copilot is intentionally excluded (no API-key concept).
pub const WIZARD_PROVIDERS: &[ProviderId] = &[
    ProviderId::OpenAI,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenRouter,
];

// ── Step 1: Finnhub API key ───────────────────────────────────────────────────

pub fn step1_finnhub_api_key(partial: &mut PartialConfig) -> anyhow::Result<()> {
    println!(
        "Finnhub provides fundamental data, earnings, and company news.\n\
         Get your free key at: https://finnhub.io/dashboard"
    );
    let existing = partial.finnhub_api_key.clone();
    let mut prompt =
        inquire::Password::new("Finnhub API key:").with_display_mode(PasswordDisplayMode::Masked);
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    } else {
        prompt = prompt.with_validator(|input: &str| {
            if input.is_empty() {
                Ok(Validation::Invalid("Value is required".into()))
            } else {
                Ok(Validation::Valid)
            }
        });
    }
    let input = prompt.prompt().map_err(|e| anyhow::anyhow!(e))?;
    partial.finnhub_api_key = apply_optional_secret(&input, existing);
    Ok(())
}

// ── Step 2: FRED API key ──────────────────────────────────────────────────────

pub fn step2_fred_api_key(partial: &mut PartialConfig) -> anyhow::Result<()> {
    println!(
        "FRED provides macro indicators (CPI, inflation, interest rates).\n\
         Get your free key at: https://fredaccount.stlouisfed.org/apikeys"
    );
    let existing = partial.fred_api_key.clone();
    let mut prompt =
        inquire::Password::new("FRED API key:").with_display_mode(PasswordDisplayMode::Masked);
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    } else {
        prompt = prompt.with_validator(|input: &str| {
            if input.is_empty() {
                Ok(Validation::Invalid("Value is required".into()))
            } else {
                Ok(Validation::Valid)
            }
        });
    }
    let input = prompt.prompt().map_err(|e| anyhow::anyhow!(e))?;
    partial.fred_api_key = apply_optional_secret(&input, existing);
    Ok(())
}

// ── Step 3: LLM provider keys ─────────────────────────────────────────────────

pub fn step3_llm_provider_keys(partial: &mut PartialConfig) -> anyhow::Result<()> {
    loop {
        // Build the list of providers not yet configured.
        let available: Vec<ProviderId> = WIZARD_PROVIDERS
            .iter()
            .copied()
            .filter(|p| provider_key(partial, *p).is_none())
            .collect();

        // All providers already have keys — nothing more to do.
        if available.is_empty() {
            break;
        }

        let provider = inquire::Select::new(
            "Select an LLM provider to configure:",
            available.iter().map(|p| p.to_string()).collect(),
        )
        .prompt()
        .map_err(|e| anyhow::anyhow!(e))?;

        // Map the display string back to a ProviderId.
        let chosen = available
            .into_iter()
            .find(|p| p.to_string() == provider)
            .expect("selected provider must be in available list");

        let input = inquire::Password::new(&format!("{chosen} API key:"))
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_validator(|s: &str| {
                if s.is_empty() {
                    Ok(Validation::Invalid("Value is required".into()))
                } else {
                    Ok(Validation::Valid)
                }
            })
            .prompt()
            .map_err(|e| anyhow::anyhow!(e))?;

        set_provider_key(partial, chosen, apply_optional_secret(&input, None));

        if validate_step3_result(partial).is_err() {
            // Haven't met the minimum yet — loop without asking "more?".
            println!("✗ At least one LLM provider is required.");
            continue;
        }

        // Minimum met. Offer to add another if unconfigured providers remain.
        let still_available: Vec<_> = WIZARD_PROVIDERS
            .iter()
            .copied()
            .filter(|p| provider_key(partial, *p).is_none())
            .collect();

        if still_available.is_empty() {
            break;
        }

        let add_more = inquire::Confirm::new("Do you want to add another provider key?")
            .with_default(false)
            .prompt()
            .map_err(|e| anyhow::anyhow!(e))?;

        if !add_more {
            break;
        }
    }
    Ok(())
}

// ── Step 4: Provider routing ──────────────────────────────────────────────────

pub fn step4_provider_routing(partial: &mut PartialConfig) -> anyhow::Result<()> {
    let eligible = providers_with_keys(partial);
    let eligible_names: Vec<String> = eligible.iter().map(|p| p.to_string()).collect();

    // Quick-thinking provider
    let qt_default_idx = partial
        .quick_thinking_provider
        .as_deref()
        .and_then(|name| eligible.iter().position(|p| p.as_str() == name))
        .unwrap_or(0);

    let qt_provider_str = inquire::Select::new(
        "Quick-thinking provider (used by analyst agents):",
        eligible_names.clone(),
    )
    .with_starting_cursor(qt_default_idx)
    .prompt()
    .map_err(|e| anyhow::anyhow!(e))?;

    let qt_model = inquire::Text::new("Quick-thinking model:")
        .with_initial_value(partial.quick_thinking_model.as_deref().unwrap_or(""))
        .with_validator(|s: &str| {
            if s.trim().is_empty() {
                Ok(Validation::Invalid("Model name must not be empty".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()
        .map_err(|e| anyhow::anyhow!(e))?;

    // Deep-thinking provider
    let dt_default_idx = partial
        .deep_thinking_provider
        .as_deref()
        .and_then(|name| eligible.iter().position(|p| p.as_str() == name))
        .unwrap_or(0);

    let dt_provider_str = inquire::Select::new(
        "Deep-thinking provider (used by researcher, trader, and risk agents):",
        eligible_names,
    )
    .with_starting_cursor(dt_default_idx)
    .prompt()
    .map_err(|e| anyhow::anyhow!(e))?;

    let dt_model = inquire::Text::new("Deep-thinking model:")
        .with_initial_value(partial.deep_thinking_model.as_deref().unwrap_or(""))
        .with_validator(|s: &str| {
            if s.trim().is_empty() {
                Ok(Validation::Invalid("Model name must not be empty".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()
        .map_err(|e| anyhow::anyhow!(e))?;

    let qt_id = eligible
        .iter()
        .find(|p| p.to_string() == qt_provider_str)
        .copied()
        .expect("selected quick-thinking provider must be in eligible list");
    let dt_id = eligible
        .iter()
        .find(|p| p.to_string() == dt_provider_str)
        .copied()
        .expect("selected deep-thinking provider must be in eligible list");

    apply_provider_routing(partial, (qt_id, qt_model), (dt_id, dt_model));
    Ok(())
}

// ── Step 5: LLM health check ──────────────────────────────────────────────────

/// Run a single `"Hello"` prompt through the configured deep-thinking provider.
///
/// Returns `Ok(true)` when the config should be saved (health check passed, or
/// user confirmed "Save anyway?"). Returns `Ok(false)` when the health check
/// failed and the user declined to save.
pub fn step5_health_check(partial: &PartialConfig) -> anyhow::Result<bool> {
    let deep_provider = partial.deep_thinking_provider.as_deref().unwrap_or("");
    let deep_model = partial.deep_thinking_model.as_deref().unwrap_or("");
    println!("Sending \"Hello\" to deep-thinking provider ({deep_provider} / {deep_model})...");

    let llm = crate::config::LlmConfig {
        quick_thinking_provider: partial.quick_thinking_provider.clone().unwrap_or_default(),
        deep_thinking_provider: deep_provider.to_owned(),
        quick_thinking_model: partial.quick_thinking_model.clone().unwrap_or_default(),
        deep_thinking_model: deep_model.to_owned(),
        max_debate_rounds: 1,
        max_risk_rounds: 1,
        analyst_timeout_secs: HEALTH_CHECK_TIMEOUT_SECS,
        valuation_fetch_timeout_secs: HEALTH_CHECK_TIMEOUT_SECS,
        retry_max_retries: 1,
        retry_base_delay_ms: 500,
    };

    let providers = build_providers_config_from_partial(partial);
    let rate_limiters = crate::rate_limit::ProviderRateLimiters::from_config(&providers);

    let handle = crate::providers::factory::create_completion_model(
        ModelTier::DeepThinking,
        &llm,
        &providers,
        &rate_limiters,
    )
    .map_err(|e| anyhow::anyhow!("failed to create completion model: {e}"))?;

    let agent = crate::providers::factory::build_agent(&handle, "");

    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for health check")?
        .block_on(crate::providers::factory::prompt_with_retry(
            &agent,
            "Hello",
            Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS),
            &RetryPolicy::default(),
        ));

    match result {
        Ok(_) => {
            println!("✓ Health check passed.");
            Ok(true)
        }
        Err(e) => {
            eprintln!("✗ Health check failed: {e}");
            let save_anyway = inquire::Confirm::new("Save config anyway?")
                .with_default(false)
                .prompt()
                .map_err(|e| anyhow::anyhow!(e))?;
            Ok(save_anyway)
        }
    }
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

/// Return `Some(input)` when `input` is non-empty, otherwise preserve `current`.
///
/// This is the "press Enter to keep" semantic: an empty string from the prompt
/// means "keep what was there before", while any non-empty string replaces it.
pub(super) fn apply_optional_secret(input: &str, current: Option<String>) -> Option<String> {
    if input.is_empty() {
        current
    } else {
        Some(input.to_owned())
    }
}

/// Write all four provider-routing fields into `partial` atomically.
pub(super) fn apply_provider_routing(
    partial: &mut PartialConfig,
    quick: (ProviderId, String),
    deep: (ProviderId, String),
) {
    partial.quick_thinking_provider = Some(quick.0.as_str().to_owned());
    partial.quick_thinking_model = Some(quick.1);
    partial.deep_thinking_provider = Some(deep.0.as_str().to_owned());
    partial.deep_thinking_model = Some(deep.1);
}

/// Return `Err` when no LLM provider key is present in `partial`.
pub(super) fn validate_step3_result(partial: &PartialConfig) -> Result<(), &'static str> {
    if partial.openai_api_key.is_none()
        && partial.anthropic_api_key.is_none()
        && partial.gemini_api_key.is_none()
        && partial.openrouter_api_key.is_none()
    {
        Err("At least one LLM provider is required.")
    } else {
        Ok(())
    }
}

/// Return the subset of `WIZARD_PROVIDERS` that have a non-`None` key in `partial`,
/// preserving declaration order.
pub(super) fn providers_with_keys(partial: &PartialConfig) -> Vec<ProviderId> {
    WIZARD_PROVIDERS
        .iter()
        .copied()
        .filter(|p| provider_key(partial, *p).is_some())
        .collect()
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn provider_key(partial: &PartialConfig, provider: ProviderId) -> Option<&str> {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key.as_deref(),
        ProviderId::Anthropic => partial.anthropic_api_key.as_deref(),
        ProviderId::Gemini => partial.gemini_api_key.as_deref(),
        ProviderId::OpenRouter => partial.openrouter_api_key.as_deref(),
        ProviderId::Copilot => None,
    }
}

fn set_provider_key(partial: &mut PartialConfig, provider: ProviderId, value: Option<String>) {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key = value,
        ProviderId::Anthropic => partial.anthropic_api_key = value,
        ProviderId::Gemini => partial.gemini_api_key = value,
        ProviderId::OpenRouter => partial.openrouter_api_key = value,
        ProviderId::Copilot => {}
    }
}

fn build_providers_config_from_partial(partial: &PartialConfig) -> crate::config::ProvidersConfig {
    use secrecy::SecretString;
    let mut providers = crate::config::ProvidersConfig::default();
    if let Some(k) = &partial.openai_api_key {
        providers.openai.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.anthropic_api_key {
        providers.anthropic.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.gemini_api_key {
        providers.gemini.api_key = Some(SecretString::from(k.clone()));
    }
    if let Some(k) = &partial.openrouter_api_key {
        providers.openrouter.api_key = Some(SecretString::from(k.clone()));
    }
    providers
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn partial_with_openai() -> PartialConfig {
        PartialConfig {
            openai_api_key: Some("sk-openai".into()),
            ..Default::default()
        }
    }

    // ── apply_optional_secret ─────────────────────────────────────────────────

    #[test]
    fn apply_optional_secret_empty_input_none_current_stays_none() {
        assert_eq!(apply_optional_secret("", None), None);
    }

    #[test]
    fn apply_optional_secret_non_empty_input_none_current_becomes_some() {
        assert_eq!(apply_optional_secret("abc", None), Some("abc".to_owned()));
    }

    #[test]
    fn apply_optional_secret_empty_input_some_current_keeps_current() {
        assert_eq!(
            apply_optional_secret("", Some("old".to_owned())),
            Some("old".to_owned())
        );
    }

    #[test]
    fn apply_optional_secret_non_empty_input_some_current_replaces() {
        assert_eq!(
            apply_optional_secret("new", Some("old".to_owned())),
            Some("new".to_owned())
        );
    }

    // ── apply_provider_routing ────────────────────────────────────────────────

    #[test]
    fn apply_provider_routing_writes_all_four_fields() {
        let mut partial = PartialConfig::default();
        apply_provider_routing(
            &mut partial,
            (ProviderId::OpenAI, "gpt-4o-mini".into()),
            (ProviderId::Anthropic, "claude-opus-4-5".into()),
        );
        assert_eq!(partial.quick_thinking_provider.as_deref(), Some("openai"));
        assert_eq!(partial.quick_thinking_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(partial.deep_thinking_provider.as_deref(), Some("anthropic"));
        assert_eq!(
            partial.deep_thinking_model.as_deref(),
            Some("claude-opus-4-5")
        );
    }

    // ── validate_step3_result ─────────────────────────────────────────────────

    #[test]
    fn validate_step3_result_all_none_returns_err() {
        assert!(validate_step3_result(&PartialConfig::default()).is_err());
    }

    #[test]
    fn validate_step3_result_openai_key_returns_ok() {
        assert!(validate_step3_result(&partial_with_openai()).is_ok());
    }

    #[test]
    fn validate_step3_result_anthropic_key_returns_ok() {
        let p = PartialConfig {
            anthropic_api_key: Some("sk-ant".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p).is_ok());
    }

    #[test]
    fn validate_step3_result_gemini_key_returns_ok() {
        let p = PartialConfig {
            gemini_api_key: Some("AIza".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p).is_ok());
    }

    #[test]
    fn validate_step3_result_openrouter_key_returns_ok() {
        let p = PartialConfig {
            openrouter_api_key: Some("or-key".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p).is_ok());
    }

    // ── providers_with_keys ───────────────────────────────────────────────────

    #[test]
    fn providers_with_keys_empty_partial_returns_empty() {
        assert!(providers_with_keys(&PartialConfig::default()).is_empty());
    }

    #[test]
    fn providers_with_keys_preserves_declaration_order() {
        let p = PartialConfig {
            gemini_api_key: Some("g".into()),
            openai_api_key: Some("o".into()),
            ..Default::default()
        };
        let result = providers_with_keys(&p);
        // WIZARD_PROVIDERS order: OpenAI, Anthropic, Gemini, OpenRouter
        assert_eq!(result, vec![ProviderId::OpenAI, ProviderId::Gemini]);
    }

    #[test]
    fn providers_with_keys_all_set_returns_all_wizard_providers() {
        let p = PartialConfig {
            openai_api_key: Some("o".into()),
            anthropic_api_key: Some("a".into()),
            gemini_api_key: Some("g".into()),
            openrouter_api_key: Some("r".into()),
            ..Default::default()
        };
        assert_eq!(providers_with_keys(&p), WIZARD_PROVIDERS.to_vec());
    }

    // ── ProviderId Display ────────────────────────────────────────────────────

    #[test]
    fn provider_id_display_matches_as_str() {
        for &p in WIZARD_PROVIDERS {
            assert_eq!(p.to_string(), p.as_str());
        }
    }
}
