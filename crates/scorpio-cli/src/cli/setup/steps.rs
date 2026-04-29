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

use scorpio_core::constants::HEALTH_CHECK_TIMEOUT_SECS;
use scorpio_core::error::RetryPolicy;
use scorpio_core::providers::{ModelTier, ProviderId};
use scorpio_core::settings::PartialConfig;

/// Wizard-visible providers — Copilot is intentionally excluded (no API-key concept).
pub const WIZARD_PROVIDERS: &[ProviderId] = &[
    ProviderId::OpenAI,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenRouter,
    ProviderId::DeepSeek,
];

// ── Step 1: Finnhub API key ───────────────────────────────────────────────────

/// Prompt for the Finnhub API key, preserving an existing saved value on empty input.
pub fn step1_finnhub_api_key(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    println!(
        "Finnhub provides fundamental data, earnings, and company news.\n\
         Get your free key at: https://finnhub.io/dashboard"
    );
    let existing = partial.finnhub_api_key.clone();
    let mut prompt = inquire::Password::new("Finnhub API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    }
    if secret_step_requires_input(existing.as_deref(), true) {
        prompt = prompt.with_validator(|input: &str| {
            if input.is_empty() {
                Ok(Validation::Invalid("Value is required".into()))
            } else {
                Ok(Validation::Valid)
            }
        });
    }
    let input = prompt.prompt()?;
    partial.finnhub_api_key = apply_optional_secret(&input, existing);
    Ok(())
}

// ── Step 2: FRED API key ──────────────────────────────────────────────────────

/// Prompt for the optional FRED API key, preserving an existing saved value on empty input.
pub fn step2_fred_api_key(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    println!(
        "FRED provides macro indicators (CPI, inflation, interest rates).\n\
         Get your free key at: https://fredaccount.stlouisfed.org/apikeys"
    );
    let existing = partial.fred_api_key.clone();
    let mut prompt = inquire::Password::new("FRED API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    }
    if secret_step_requires_input(existing.as_deref(), false) {
        prompt = prompt.with_validator(|input: &str| {
            if input.is_empty() {
                Ok(Validation::Invalid("Value is required".into()))
            } else {
                Ok(Validation::Valid)
            }
        });
    }
    let input = prompt.prompt()?;
    partial.fred_api_key = apply_optional_secret(&input, existing);
    Ok(())
}

// ── Step 3: LLM provider keys ─────────────────────────────────────────────────

/// Prompt for one or more LLM provider keys, requiring at least one configured provider.
pub fn step3_llm_provider_keys(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    loop {
        let provider = inquire::Select::new(
            "Select an LLM provider to configure:",
            provider_choices(partial),
        )
        .prompt()?;

        let chosen = provider.provider;
        let existing = provider_key(partial, chosen).map(str::to_owned);

        let prompt_label = format!("{chosen} API key:");
        let mut prompt = inquire::Password::new(&prompt_label)
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation();
        if existing.is_some() {
            prompt = prompt.with_help_message("[already set — press Enter to keep]");
        }
        if secret_step_requires_input(existing.as_deref(), true) {
            prompt = prompt.with_validator(|s: &str| {
                if s.is_empty() {
                    Ok(Validation::Invalid("Value is required".into()))
                } else {
                    Ok(Validation::Valid)
                }
            });
        }
        let input = prompt.prompt()?;

        set_provider_key(partial, chosen, apply_optional_secret(&input, existing));

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
            .prompt()?;

        if !add_more {
            break;
        }

        if providers_with_keys(partial).len() == WIZARD_PROVIDERS.len() {
            break;
        }
    }
    Ok(())
}

// ── Step 4: Provider routing ──────────────────────────────────────────────────

/// Prompt for quick/deep provider routing using only providers that have saved keys.
pub fn step4_provider_routing(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    let eligible = providers_with_keys(partial);

    // Quick-thinking provider
    let qt_default_idx = partial
        .quick_thinking_provider
        .as_deref()
        .and_then(|name| eligible.iter().position(|p| p.as_str() == name))
        .unwrap_or(0);

    let qt_provider = inquire::Select::new(
        "Quick-thinking provider (used by analyst agents):",
        eligible.clone(),
    )
    .with_starting_cursor(qt_default_idx)
    .prompt()?;

    let qt_model = inquire::Text::new("Quick-thinking model:")
        .with_initial_value(partial.quick_thinking_model.as_deref().unwrap_or(""))
        .with_validator(|s: &str| {
            if s.trim().is_empty() {
                Ok(Validation::Invalid("Model name must not be empty".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()?;

    // Deep-thinking provider
    let dt_default_idx = partial
        .deep_thinking_provider
        .as_deref()
        .and_then(|name| eligible.iter().position(|p| p.as_str() == name))
        .unwrap_or(0);

    let dt_provider = inquire::Select::new(
        "Deep-thinking provider (used by researcher, trader, and risk agents):",
        eligible,
    )
    .with_starting_cursor(dt_default_idx)
    .prompt()?;

    let dt_model = inquire::Text::new("Deep-thinking model:")
        .with_initial_value(partial.deep_thinking_model.as_deref().unwrap_or(""))
        .with_validator(|s: &str| {
            if s.trim().is_empty() {
                Ok(Validation::Invalid("Model name must not be empty".into()))
            } else {
                Ok(Validation::Valid)
            }
        })
        .prompt()?;

    apply_provider_routing(partial, (qt_provider, qt_model), (dt_provider, dt_model));
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

    let cfg = scorpio_core::config::Config::load_effective_runtime(partial.clone())?;

    run_health_check_loop(
        || run_single_health_check(&cfg),
        |error| {
            eprintln!(
                "✗ Health check failed: {}",
                scorpio_core::providers::factory::sanitize_error_summary(&error.to_string())
            );
        },
        || {
            inquire::Confirm::new("Retry health check?")
                .with_default(true)
                .prompt()
                .map_err(anyhow::Error::from)
        },
        || {
            inquire::Confirm::new("Save config anyway?")
                .with_default(false)
                .prompt()
                .map_err(anyhow::Error::from)
        },
    )
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
        && partial.deepseek_api_key.is_none()
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

fn provider_choices(partial: &PartialConfig) -> Vec<ProviderChoice> {
    WIZARD_PROVIDERS
        .iter()
        .copied()
        .map(|provider| ProviderChoice {
            provider,
            label: if provider_key(partial, provider).is_some() {
                format!("{provider} [already set]")
            } else {
                provider.to_string()
            },
        })
        .collect()
}

pub(super) const fn secret_step_requires_input(
    existing: Option<&str>,
    required_on_first_run: bool,
) -> bool {
    existing.is_none() && required_on_first_run
}

pub(super) fn run_health_check_loop<Run, Report, Retry, Save>(
    mut run_check: Run,
    mut report_failure: Report,
    mut should_retry: Retry,
    mut should_save_anyway: Save,
) -> anyhow::Result<bool>
where
    Run: FnMut() -> anyhow::Result<()>,
    Report: FnMut(&anyhow::Error),
    Retry: FnMut() -> anyhow::Result<bool>,
    Save: FnMut() -> anyhow::Result<bool>,
{
    loop {
        return match run_check() {
            Ok(()) => {
                println!("✓ Health check passed.");
                Ok(true)
            }
            Err(error) => {
                report_failure(&error);
                if should_retry()? {
                    continue;
                }
                should_save_anyway()
            }
        };
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderChoice {
    provider: ProviderId,
    label: String,
}

impl std::fmt::Display for ProviderChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

fn provider_key(partial: &PartialConfig, provider: ProviderId) -> Option<&str> {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key.as_deref(),
        ProviderId::Anthropic => partial.anthropic_api_key.as_deref(),
        ProviderId::Gemini => partial.gemini_api_key.as_deref(),
        ProviderId::OpenRouter => partial.openrouter_api_key.as_deref(),
        ProviderId::DeepSeek => partial.deepseek_api_key.as_deref(),
    }
}

fn set_provider_key(partial: &mut PartialConfig, provider: ProviderId, value: Option<String>) {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key = value,
        ProviderId::Anthropic => partial.anthropic_api_key = value,
        ProviderId::Gemini => partial.gemini_api_key = value,
        ProviderId::OpenRouter => partial.openrouter_api_key = value,
        ProviderId::DeepSeek => partial.deepseek_api_key = value,
    }
}

fn check_selected_model_tiers<I, Check>(tiers: I, mut check: Check) -> anyhow::Result<()>
where
    I: IntoIterator<Item = ModelTier>,
    Check: FnMut(ModelTier) -> anyhow::Result<()>,
{
    for tier in tiers {
        check(tier).with_context(|| format!("{tier} model health check failed"))?;
    }

    Ok(())
}

fn run_single_health_check(cfg: &scorpio_core::config::Config) -> anyhow::Result<()> {
    let rate_limiters = scorpio_core::rate_limit::ProviderRateLimiters::from_config(&cfg.providers);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for health check")?;

    cfg.is_analysis_ready()
        .context("effective runtime config is not ready for analysis")?;

    check_selected_model_tiers(
        [ModelTier::QuickThinking, ModelTier::DeepThinking],
        |tier| {
            let handle = scorpio_core::providers::factory::create_completion_model(
                tier,
                &cfg.llm,
                &cfg.providers,
                &rate_limiters,
            )
            .map_err(|e| anyhow::anyhow!("failed to create completion model: {e}"))?;

            runtime
                .block_on(async {
                    // build_agent calls ToolServer::new().run() → tokio::spawn internally,
                    // so it must be called from within a live Tokio runtime context.
                    let agent = scorpio_core::providers::factory::build_agent(&handle, "");
                    scorpio_core::providers::factory::prompt_with_retry(
                        &agent,
                        "Hello",
                        Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS),
                        &RetryPolicy::default(),
                    )
                    .await
                })
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!(e))
        },
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            deepseek_api_key: Some("d".into()),
            ..Default::default()
        };
        assert_eq!(providers_with_keys(&p), WIZARD_PROVIDERS.to_vec());
    }

    // ── DeepSeek provider tests (Task C) ─────────────────────────────────────

    #[test]
    fn validate_step3_result_deepseek_key_returns_ok() {
        let p = PartialConfig {
            deepseek_api_key: Some("sk-deepseek".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p).is_ok());
    }

    #[test]
    fn provider_key_and_set_provider_key_handle_deepseek() {
        let mut partial = PartialConfig::default();
        set_provider_key(
            &mut partial,
            ProviderId::DeepSeek,
            Some("sk-deepseek".into()),
        );
        assert_eq!(
            provider_key(&partial, ProviderId::DeepSeek),
            Some("sk-deepseek")
        );
    }

    #[test]
    fn providers_with_keys_includes_deepseek_in_declaration_order() {
        let p = PartialConfig {
            openai_api_key: Some("o".into()),
            deepseek_api_key: Some("d".into()),
            ..Default::default()
        };
        assert_eq!(
            providers_with_keys(&p),
            vec![ProviderId::OpenAI, ProviderId::DeepSeek]
        );
    }

    #[test]
    fn provider_labels_include_already_set_marker_on_rerun() {
        let p = PartialConfig {
            openai_api_key: Some("o".into()),
            ..Default::default()
        };

        let labels: Vec<String> = provider_choices(&p)
            .into_iter()
            .map(|choice| choice.to_string())
            .collect();

        assert!(
            labels.contains(&"openai [already set]".to_owned()),
            "rerun choices should keep already-configured providers selectable"
        );
        assert!(
            labels.contains(&"anthropic".to_owned()),
            "unset providers should still be available"
        );
    }

    #[test]
    fn required_secret_step_requires_input_on_first_run() {
        assert!(secret_step_requires_input(None, true));
    }

    #[test]
    fn optional_secret_step_does_not_require_input_on_first_run() {
        assert!(!secret_step_requires_input(None, false));
    }

    #[test]
    fn load_effective_runtime_uses_env_secret_overrides() {
        let _guard = ENV_LOCK.lock().unwrap();
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-from-file".into()),
            finnhub_api_key: Some("fh-file".into()),
            fred_api_key: Some("fred-file".into()),
            ..Default::default()
        };

        unsafe {
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "sk-from-env");
        }

        let cfg = scorpio_core::config::Config::load_effective_runtime(partial.clone())
            .expect("merged config should load");

        unsafe {
            std::env::remove_var("SCORPIO_OPENAI_API_KEY");
        }

        assert_eq!(
            cfg.providers
                .openai
                .api_key
                .as_ref()
                .map(ExposeSecret::expose_secret),
            Some("sk-from-env")
        );
    }

    #[test]
    fn run_single_health_check_requires_same_analysis_readiness_as_analyze() {
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-from-file".into()),
            ..Default::default()
        };

        let cfg = scorpio_core::config::Config::load_effective_runtime(partial.clone())
            .expect("merged config should load");
        let err = run_single_health_check(&cfg)
            .expect_err("health check should fail when analyze readiness fails");

        assert!(
            err.to_string().contains("quick-thinking provider")
                || err.to_string().contains("not ready for analysis"),
            "analysis-readiness failure should be surfaced before probe: {err}"
        );
    }

    #[test]
    fn check_selected_model_tiers_runs_quick_then_deep() {
        let mut seen = Vec::new();

        check_selected_model_tiers(
            [ModelTier::QuickThinking, ModelTier::DeepThinking],
            |tier| {
                seen.push(tier);
                Ok(())
            },
        )
        .expect("both tier checks should succeed");

        assert_eq!(
            seen,
            vec![ModelTier::QuickThinking, ModelTier::DeepThinking]
        );
    }

    #[test]
    fn check_selected_model_tiers_stops_after_quick_failure() {
        let mut seen = Vec::new();

        let err = check_selected_model_tiers(
            [ModelTier::QuickThinking, ModelTier::DeepThinking],
            |tier| {
                seen.push(tier);
                match tier {
                    ModelTier::QuickThinking => anyhow::bail!("quick tier failed"),
                    ModelTier::DeepThinking => Ok(()),
                }
            },
        )
        .expect_err("quick-tier failure should abort later checks");

        assert_eq!(seen, vec![ModelTier::QuickThinking]);
        assert!(
            err.to_string()
                .contains("quick-thinking model health check failed"),
            "tier failure should be annotated: {err:#}"
        );
    }

    #[test]
    fn run_health_check_loop_retries_then_succeeds() {
        let mut attempts = 0;

        let should_save = run_health_check_loop(
            || {
                attempts += 1;
                if attempts == 1 {
                    anyhow::bail!("transient failure")
                }
                Ok(())
            },
            |_err| {},
            || Ok(true),
            || Ok(false),
        )
        .expect("retry flow should succeed");

        assert!(should_save);
        assert_eq!(
            attempts, 2,
            "health check should retry once before succeeding"
        );
    }

    #[test]
    fn run_health_check_loop_can_save_anyway_after_failure() {
        let mut attempts = 0;

        let should_save = run_health_check_loop(
            || {
                attempts += 1;
                anyhow::bail!("persistent failure")
            },
            |_err| {},
            || Ok(false),
            || Ok(true),
        )
        .expect("save-anyway flow should succeed");

        assert!(should_save);
        assert_eq!(attempts, 1, "declining retry should skip additional probes");
    }

    #[test]
    fn run_health_check_loop_can_abort_after_failure() {
        let should_save = run_health_check_loop(
            || anyhow::bail!("persistent failure"),
            |_err| {},
            || Ok(false),
            || Ok(false),
        )
        .expect("abort flow should succeed");

        assert!(!should_save);
    }

    // ── ProviderId Display ────────────────────────────────────────────────────

    #[test]
    fn provider_id_display_matches_as_str() {
        for &p in WIZARD_PROVIDERS {
            assert_eq!(p.to_string(), p.as_str());
        }
    }
}
