//! Interactive setup wizard step functions.
//!
//! Each public `stepN_*` function drives one wizard step via `inquire` prompts and
//! delegates state mutations to a pure `apply_*` / `validate_*` helper so the logic
//! can be unit-tested without touching stdin.
//!
//! **Step 3 — Copilot:** The provider selection list now includes `copilot`. If a valid
//! identity binding already exists on disk the entry is labelled `[already set]`; otherwise
//! selecting it runs the GitHub OAuth device flow and writes the binding immediately.
//! Copilot auth in step 3 satisfies the "at least one provider" requirement on its own.
//!
//! **Testing note:** The `step5_health_check` function calls a real LLM via
//! `prompt_with_retry` and cannot be driven in unit tests; it is covered by manual
//! QA and the Unit 5 smoke test. The probe/retry/Langfuse internals it
//! delegates to live in [`super::health_check`] alongside their unit tests.

use anyhow::Context;
use inquire::{PasswordDisplayMode, validator::Validation};

use scorpio_core::config::ProvidersConfig;
use scorpio_core::providers::ProviderId;
use scorpio_core::settings::PartialConfig;

use super::health_check::{
    best_effort_harden_copilot_cache_files, configured_non_copilot_tiers,
    describe_health_check_targets, effective_copilot_tiers, report_langfuse_health_check,
    run_copilot_auth_loop, run_copilot_auth_only, run_copilot_model_probe, run_health_check_loop,
    run_selected_model_tiers, run_single_health_check, step5_validate_copilot_auth,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StepThreeOutcome {
    pub copilot_only: bool,
}

/// Step-3 keyed providers — those for which the wizard prompts for an API key.
/// Copilot is intentionally excluded (it uses OAuth, not an API key).
pub const KEYED_WIZARD_PROVIDERS: &[ProviderId] = &[
    ProviderId::OpenAI,
    ProviderId::Anthropic,
    ProviderId::Gemini,
    ProviderId::OpenRouter,
    ProviderId::DeepSeek,
    ProviderId::XiaomiMimo,
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

// ── Step 2b: Alpha Vantage API key ───────────────────────────────────────────

/// Prompt for the optional Alpha Vantage API key, preserving an existing saved value on empty input.
pub fn step2b_alpha_vantage_api_key(
    partial: &mut PartialConfig,
) -> Result<(), inquire::InquireError> {
    println!(
        "Alpha Vantage provides earnings call transcripts.\n\
         Get your free key at: https://www.alphavantage.co/support/#api-key\n\
         Free tier: 25 requests/day."
    );
    let existing = partial.alpha_vantage_api_key.clone();
    let mut prompt = inquire::Password::new("Alpha Vantage API key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing.is_some() {
        prompt = prompt.with_help_message("[already set — press Enter to keep]");
    }
    let input = prompt.prompt()?;
    partial.alpha_vantage_api_key = apply_optional_secret(&input, existing);
    Ok(())
}

// ── Step 3: LLM provider keys ─────────────────────────────────────────────────

/// Prompt for one or more LLM provider keys, requiring at least one configured provider.
pub(crate) fn step3_llm_provider_keys(
    config_path: &std::path::Path,
    partial: &mut PartialConfig,
) -> Result<StepThreeOutcome, inquire::InquireError> {
    let effective_providers =
        scorpio_core::config::Config::load_effective_providers_config_from_user_path(
            config_path,
            partial,
        )
        .unwrap_or_default();

    if providers_with_keys(partial, &effective_providers).is_empty()
        && inquire::Confirm::new("No LLM provider keys found. Continue with GitHub Copilot only?")
            .with_default(true)
            .prompt()?
    {
        return Ok(StepThreeOutcome { copilot_only: true });
    }

    let mut copilot_authed_in_step3 = is_copilot_authenticated();

    loop {
        let mut items: Vec<ProviderSelectItem> = provider_choices(partial)
            .into_iter()
            .map(ProviderSelectItem::Provider)
            .collect();
        items.push(ProviderSelectItem::Copilot {
            already_authed: copilot_authed_in_step3,
        });
        items.push(ProviderSelectItem::Skip);

        let selection =
            inquire::Select::new("Select an LLM provider to configure:", items).prompt()?;

        match selection {
            ProviderSelectItem::Skip => {
                if validate_step3_result(
                    partial,
                    &effective_providers,
                    false,
                    copilot_authed_in_step3,
                )
                .is_ok()
                {
                    break;
                }
                println!("✗ At least one LLM provider is required.");
                continue;
            }
            ProviderSelectItem::Copilot { already_authed } => {
                if already_authed {
                    println!("✓ Copilot already authorized.");
                    copilot_authed_in_step3 = true;
                } else {
                    handle_copilot_selection_in_step3(&mut copilot_authed_in_step3)?;
                }
                if validate_step3_result(
                    partial,
                    &effective_providers,
                    false,
                    copilot_authed_in_step3,
                )
                .is_ok()
                {
                    let add_more =
                        inquire::Confirm::new("Do you want to add another provider key?")
                            .with_default(false)
                            .prompt()?;
                    if !add_more {
                        break;
                    }
                }
                continue;
            }
            ProviderSelectItem::Provider(c) => {
                let chosen = c.provider;
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

                if validate_step3_result(
                    partial,
                    &effective_providers,
                    false,
                    copilot_authed_in_step3,
                )
                .is_err()
                {
                    println!("✗ At least one LLM provider is required.");
                    continue;
                }

                // Minimum met. Offer to add another if unconfigured providers remain.
                let still_available: Vec<_> = KEYED_WIZARD_PROVIDERS
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

                if providers_with_keys(partial, &effective_providers).len()
                    == KEYED_WIZARD_PROVIDERS.len()
                {
                    break;
                }
            }
        }
    }

    // copilot_only: true when Copilot was the only configured provider
    let copilot_only =
        copilot_authed_in_step3 && providers_with_keys(partial, &effective_providers).is_empty();

    Ok(StepThreeOutcome { copilot_only })
}

// ── Step 4: Provider routing ──────────────────────────────────────────────────

/// Prompt for quick/deep provider routing using providers that have saved keys, plus Copilot.
pub(crate) fn step4_provider_routing(
    config_path: &std::path::Path,
    partial: &mut PartialConfig,
    step3_outcome: &StepThreeOutcome,
) -> Result<(), inquire::InquireError> {
    let effective_providers =
        scorpio_core::config::Config::load_effective_providers_config_from_user_path(
            config_path,
            partial,
        )
        .unwrap_or_default();
    let eligible = eligible_routing_providers(partial, &effective_providers);
    let defaults = default_routing_from_step3(step3_outcome);
    if defaults.keyed_providers_skipped_message {
        println!(
            "Keyed providers were skipped for this run. Copilot is preselected for both tiers; rerun setup later to add provider keys."
        );
    }
    super::model_selection::prompt_provider_routing(partial, eligible, config_path, &defaults)
}

// ── Step 4b: Langfuse observability (optional) ───────────────────────────────

/// Prompt for the three optional Langfuse credentials used for OTel tracing.
///
/// All three values are optional: leaving any prompt blank preserves the
/// previously-saved value (or `None` if nothing was set). Langfuse export only
/// activates when public key, secret key, and base URL are all present.
pub fn step_langfuse_observability(
    partial: &mut PartialConfig,
) -> Result<(), inquire::InquireError> {
    println!(
        "Langfuse provides OpenTelemetry tracing for LLM calls (optional).\n\
         Sign up at: https://cloud.langfuse.com (or use a self-hosted base URL).\n\
         Leave any prompt blank to skip / keep the existing value."
    );

    let existing_public = partial.langfuse_public_key.clone();
    let mut public_prompt = inquire::Password::new("Langfuse public key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing_public.is_some() {
        public_prompt = public_prompt.with_help_message("[already set — press Enter to keep]");
    }
    let public_input = public_prompt.prompt()?;
    partial.langfuse_public_key = apply_optional_secret(&public_input, existing_public);

    let existing_secret = partial.langfuse_secret_key.clone();
    let mut secret_prompt = inquire::Password::new("Langfuse secret key:")
        .with_display_mode(PasswordDisplayMode::Masked)
        .without_confirmation();
    if existing_secret.is_some() {
        secret_prompt = secret_prompt.with_help_message("[already set — press Enter to keep]");
    }
    let secret_input = secret_prompt.prompt()?;
    partial.langfuse_secret_key = apply_optional_secret(&secret_input, existing_secret);

    let existing_url = partial.langfuse_base_url.clone();
    let url_prompt = inquire::Text::new("Langfuse base URL:");
    let url_prompt = match existing_url.as_deref() {
        Some(value) => url_prompt.with_initial_value(value),
        None => url_prompt.with_placeholder("https://cloud.langfuse.com"),
    };
    let url_input = url_prompt.prompt()?;
    partial.langfuse_base_url = apply_optional_secret(url_input.trim(), existing_url);

    report_langfuse_health_check(partial);

    Ok(())
}

// ── Step 4c: Futu account positions (optional, read-only) ────────────────────

/// Prompt to enable the read-only Futu OpenD position lookup and, when enabled,
/// optionally pin a specific Real account. Default-off; disabling clears any
/// previously saved account. Empty account input means auto-select. The account
/// is matched against each Real account's universal account number (the one
/// shown in the Futu app) or raw acc_id.
pub fn step_futu_positions(partial: &mut PartialConfig) -> Result<(), inquire::InquireError> {
    println!(
        "Futu positions (optional, read-only): let the Fund Manager see your current\n\
         Real-account holdings for the analyzed symbol's market. Requires a local Futu\n\
         OpenD on 127.0.0.1:11111 with API encryption disabled.\n\
         When enabled, holdings are sent to your configured LLM provider and saved in\n\
         local run snapshots. Strictly read-only (never unlocks trading). Default off."
    );

    let enabled = inquire::Confirm::new("Enable Futu account positions?")
        .with_default(partial.futu_enabled.unwrap_or(false))
        .prompt()?;
    partial.futu_enabled = Some(enabled);

    if !enabled {
        partial.futu_account = None;
        return Ok(());
    }

    let existing = partial.futu_account.clone();
    let mut prompt = inquire::Text::new("Futu account (optional — leave blank to auto-select):")
        .with_help_message(
            "Universal account number (shown in the Futu app) or acc_id. Blank = first Real account for the market.",
        );
    if let Some(ref account) = existing {
        prompt = prompt.with_initial_value(account);
    }
    let input = prompt.prompt()?;
    partial.futu_account = normalize_optional_account(&input);
    Ok(())
}

// ── Step 5: LLM health check ──────────────────────────────────────────────────

/// Run a single `"Hello"` prompt through the configured deep-thinking provider.
///
/// Returns `Ok(true)` when the config should be saved (health check passed, or
/// user confirmed "Save anyway?"). Returns `Ok(false)` when the health check
/// failed and the user declined to save.
pub fn step5_health_check(partial: &PartialConfig) -> anyhow::Result<bool> {
    let cfg = scorpio_core::config::Config::load_effective_runtime(partial.clone())?;
    let copilot_tiers = effective_copilot_tiers(&cfg);

    if !copilot_tiers.is_empty() {
        println!(
            "Running Copilot setup auth and provider probes for: {}",
            describe_health_check_targets(&cfg)
        );

        let consent = inquire::Confirm::new(
            "Copilot setup validates a GitHub grant with read:user only. Continue?",
        )
        .with_default(true)
        .prompt()?;
        if !consent {
            return Ok(false);
        }

        let token_dir = scorpio_core::settings::ensure_copilot_token_dir()?;
        scorpio_core::settings::verify_copilot_token_dir_secure(&token_dir)?;
        let rate_limiters =
            scorpio_core::rate_limit::ProviderRateLimiters::from_config(&cfg.providers);

        // Phase A: OAuth grant + identity validation (auth-only, no LLM call).
        let auth_completed = run_copilot_auth_loop(
            || run_copilot_auth_only(&copilot_tiers, &cfg, &rate_limiters, &token_dir),
            |error| {
                eprintln!(
                    "✗ Copilot authorization failed: {}",
                    scorpio_core::providers::factory::sanitize_error_summary(&error.to_string())
                );
            },
            || {
                inquire::Confirm::new("Retry authorization?")
                    .with_default(true)
                    .prompt()
                    .map_err(anyhow::Error::from)
            },
            || {
                inquire::Confirm::new("Back without saving?")
                    .with_default(true)
                    .prompt()
                    .map_err(anyhow::Error::from)
            },
        )?;
        if !auth_completed {
            return Ok(false);
        }

        // Phase B: model probe — "Hello" through the configured Copilot model.
        // A 400 here means the selected model doesn't support the Chat Completions
        // endpoint (e.g. it requires the Responses API). Offer "Save anyway?" so the
        // user isn't blocked from saving a config that is otherwise valid.
        let copilot_probe_ok = run_health_check_loop(
            || run_copilot_model_probe(&copilot_tiers, &cfg, &rate_limiters, &token_dir),
            |error| {
                eprintln!(
                    "✗ Copilot model probe failed: {}",
                    scorpio_core::providers::factory::sanitize_error_summary(&error.to_string())
                );
            },
            || {
                inquire::Confirm::new("Retry model probe?")
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
        )?;
        if !copilot_probe_ok {
            return Ok(false);
        }

        let non_copilot_tiers = configured_non_copilot_tiers(&cfg);
        if non_copilot_tiers.is_empty() {
            return Ok(true);
        }

        return run_health_check_loop(
            || run_selected_model_tiers(&cfg, &non_copilot_tiers),
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
        );
    }

    println!(
        "Running provider probes for: {}",
        describe_health_check_targets(&cfg)
    );

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

/// Normalize the optional Futu account prompt input. Blank/whitespace yields
/// `None` (auto-select the first matching Real account); any other value is
/// trimmed and kept verbatim (it may be a universal account number or raw
/// acc_id — matched flexibly at fetch time, so no numeric validation).
pub(super) fn normalize_optional_account(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Return `Err` when no LLM provider key is present (unless `copilot_only_selected` or
/// `copilot_authed` — Copilot OAuth was completed during this wizard run).
pub(super) fn validate_step3_result(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
    copilot_only_selected: bool,
    copilot_authed: bool,
) -> Result<(), &'static str> {
    if copilot_only_selected || copilot_authed {
        return Ok(());
    }
    if providers_with_keys(partial, effective_providers).is_empty() {
        Err("At least one LLM provider is required (or pick the Copilot-only path)")
    } else {
        Ok(())
    }
}

/// Return the subset of `KEYED_WIZARD_PROVIDERS` that have a non-`None` key
/// in either `partial` or `effective_providers`, preserving declaration order.
pub(super) fn providers_with_keys(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
) -> Vec<ProviderId> {
    KEYED_WIZARD_PROVIDERS
        .iter()
        .filter(|p| match **p {
            ProviderId::OpenAI => {
                effective_providers.openai.api_key.is_some() || partial.openai_api_key.is_some()
            }
            ProviderId::Anthropic => {
                effective_providers.anthropic.api_key.is_some()
                    || partial.anthropic_api_key.is_some()
            }
            ProviderId::Gemini => {
                effective_providers.gemini.api_key.is_some() || partial.gemini_api_key.is_some()
            }
            ProviderId::OpenRouter => {
                effective_providers.openrouter.api_key.is_some()
                    || partial.openrouter_api_key.is_some()
            }
            ProviderId::DeepSeek => {
                effective_providers.deepseek.api_key.is_some() || partial.deepseek_api_key.is_some()
            }
            ProviderId::XiaomiMimo => {
                effective_providers.xiaomimimo.api_key.is_some()
                    || partial.xiaomimimo_api_key.is_some()
            }
            ProviderId::Copilot => false,
        })
        .copied()
        .collect()
}

/// Step-4 routing eligibility: keyed providers with secrets, plus Copilot.
///
/// Copilot is always appended at the end so existing default-selection behaviour
/// stays stable and Copilot does not become the implicit first choice.
pub(super) fn eligible_routing_providers(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
) -> Vec<ProviderId> {
    let mut eligible = providers_with_keys(partial, effective_providers);
    eligible.push(ProviderId::Copilot);
    eligible
}

fn provider_choices(partial: &PartialConfig) -> Vec<ProviderChoice> {
    KEYED_WIZARD_PROVIDERS
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

pub(super) fn default_routing_from_step3(
    outcome: &StepThreeOutcome,
) -> super::model_selection::RoutingDefaults {
    if outcome.copilot_only {
        return super::model_selection::RoutingDefaults {
            quick_provider: Some(ProviderId::Copilot),
            deep_provider: Some(ProviderId::Copilot),
            keyed_providers_skipped_message: true,
            lock_same_model_across_tiers: true,
        };
    }

    super::model_selection::RoutingDefaults::default()
}

// ── Copilot step-3 helpers ────────────────────────────────────────────────────

/// `true` when a valid Copilot identity binding exists on disk from a prior auth.
fn is_copilot_authenticated() -> bool {
    let Ok(token_dir) = scorpio_core::settings::copilot_token_dir() else {
        return false;
    };
    scorpio_core::providers::factory::copilot_auth::read_binding(&token_dir).is_ok()
}

/// Handle the user selecting "copilot" (unauthenticated) in the step-3 provider list.
///
/// Shows the consent prompt (ESC/Ctrl-C propagates up as `InquireError`), then
/// runs the OAuth device flow and validates identity. Auth errors are printed
/// inline and `copilot_authed` is left unchanged so the loop can retry.
fn handle_copilot_selection_in_step3(
    copilot_authed: &mut bool,
) -> Result<(), inquire::InquireError> {
    let consent = inquire::Confirm::new(
        "Copilot setup validates a GitHub grant with read:user only. Continue?",
    )
    .with_default(true)
    .prompt()?;

    if !consent {
        return Ok(());
    }

    match run_copilot_setup_auth() {
        Ok(()) => {
            *copilot_authed = true;
        }
        Err(e) => {
            eprintln!(
                "✗ Copilot authorization failed: {}",
                scorpio_core::providers::factory::sanitize_error_summary(&e.to_string())
            );
        }
    }

    Ok(())
}

/// Run Copilot OAuth and identity validation without prompting the LLM.
///
/// Called from step 3 when the user selects Copilot and it is not yet auth'd.
/// Step 5 will also run auth + a "Hello" probe if Copilot is selected as a routing
/// provider, but `authorize_copilot()` is idempotent when the token cache is warm.
fn run_copilot_setup_auth() -> anyhow::Result<()> {
    let token_dir = scorpio_core::settings::ensure_copilot_token_dir()?;
    scorpio_core::settings::verify_copilot_token_dir_secure(&token_dir)?;

    let handle = scorpio_core::providers::factory::build_copilot_auth_handle(&token_dir)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for Copilot setup auth")?;

    runtime.block_on(async {
        handle
            .authorize_copilot()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        step5_validate_copilot_auth(&token_dir).await
    })?;

    best_effort_harden_copilot_cache_files(&token_dir);
    Ok(())
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

#[derive(Clone, Debug)]
enum ProviderSelectItem {
    Provider(ProviderChoice),
    Copilot { already_authed: bool },
    Skip,
}

impl std::fmt::Display for ProviderSelectItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(c) => c.fmt(f),
            Self::Copilot {
                already_authed: true,
            } => f.write_str("copilot [already set]"),
            Self::Copilot {
                already_authed: false,
            } => f.write_str("copilot"),
            Self::Skip => f.write_str("Skip this step"),
        }
    }
}

fn provider_key(partial: &PartialConfig, provider: ProviderId) -> Option<&str> {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key.as_deref(),
        ProviderId::Anthropic => partial.anthropic_api_key.as_deref(),
        ProviderId::Gemini => partial.gemini_api_key.as_deref(),
        ProviderId::OpenRouter => partial.openrouter_api_key.as_deref(),
        ProviderId::DeepSeek => partial.deepseek_api_key.as_deref(),
        ProviderId::XiaomiMimo => partial.xiaomimimo_api_key.as_deref(),
        ProviderId::Copilot => None,
    }
}

fn set_provider_key(partial: &mut PartialConfig, provider: ProviderId, value: Option<String>) {
    match provider {
        ProviderId::OpenAI => partial.openai_api_key = value,
        ProviderId::Anthropic => partial.anthropic_api_key = value,
        ProviderId::Gemini => partial.gemini_api_key = value,
        ProviderId::OpenRouter => partial.openrouter_api_key = value,
        ProviderId::DeepSeek => partial.deepseek_api_key = value,
        ProviderId::XiaomiMimo => partial.xiaomimimo_api_key = value,
        ProviderId::Copilot => {}
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_core::config::{ProviderSettings, ProvidersConfig};
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

    // ── validate_step3_result ─────────────────────────────────────────────────

    #[test]
    fn validate_step3_result_all_none_returns_err() {
        assert!(
            validate_step3_result(
                &PartialConfig::default(),
                &ProvidersConfig::default(),
                false,
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn validate_step3_result_openai_key_returns_ok() {
        assert!(
            validate_step3_result(
                &partial_with_openai(),
                &ProvidersConfig::default(),
                false,
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_step3_result_anthropic_key_returns_ok() {
        let p = PartialConfig {
            anthropic_api_key: Some("sk-ant".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false, false).is_ok());
    }

    #[test]
    fn validate_step3_result_gemini_key_returns_ok() {
        let p = PartialConfig {
            gemini_api_key: Some("AIza".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false, false).is_ok());
    }

    #[test]
    fn validate_step3_result_openrouter_key_returns_ok() {
        let p = PartialConfig {
            openrouter_api_key: Some("or-key".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false, false).is_ok());
    }

    #[test]
    fn validate_step3_result_copilot_authed_returns_ok_with_no_keys() {
        assert!(
            validate_step3_result(
                &PartialConfig::default(),
                &ProvidersConfig::default(),
                false,
                true
            )
            .is_ok()
        );
    }

    // ── providers_with_keys ───────────────────────────────────────────────────

    #[test]
    fn providers_with_keys_empty_partial_returns_empty() {
        assert!(
            providers_with_keys(&PartialConfig::default(), &ProvidersConfig::default()).is_empty()
        );
    }

    #[test]
    fn providers_with_keys_preserves_declaration_order() {
        let p = PartialConfig {
            gemini_api_key: Some("g".into()),
            openai_api_key: Some("o".into()),
            ..Default::default()
        };
        let result = providers_with_keys(&p, &ProvidersConfig::default());
        // KEYED_WIZARD_PROVIDERS order: OpenAI, Anthropic, Gemini, OpenRouter, DeepSeek, XiaomiMimo
        assert_eq!(result, vec![ProviderId::OpenAI, ProviderId::Gemini]);
    }

    #[test]
    fn providers_with_keys_all_set_returns_all_keyed_wizard_providers() {
        let p = PartialConfig {
            openai_api_key: Some("o".into()),
            anthropic_api_key: Some("a".into()),
            gemini_api_key: Some("g".into()),
            openrouter_api_key: Some("r".into()),
            deepseek_api_key: Some("d".into()),
            xiaomimimo_api_key: Some("m".into()),
            ..Default::default()
        };
        assert_eq!(
            providers_with_keys(&p, &ProvidersConfig::default()),
            KEYED_WIZARD_PROVIDERS.to_vec()
        );
    }

    // ── DeepSeek provider tests (Task C) ─────────────────────────────────────

    #[test]
    fn validate_step3_result_deepseek_key_returns_ok() {
        let p = PartialConfig {
            deepseek_api_key: Some("sk-deepseek".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false, false).is_ok());
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
            providers_with_keys(&p, &ProvidersConfig::default()),
            vec![ProviderId::OpenAI, ProviderId::DeepSeek]
        );
    }

    // ── Phase 8 Task 19: KEYED_WIZARD_PROVIDERS + eligible_routing_providers ─

    #[test]
    fn keyed_wizard_providers_excludes_copilot() {
        assert!(!KEYED_WIZARD_PROVIDERS.contains(&ProviderId::Copilot));
        assert!(KEYED_WIZARD_PROVIDERS.contains(&ProviderId::OpenAI));
        assert!(KEYED_WIZARD_PROVIDERS.contains(&ProviderId::XiaomiMimo));
    }

    #[test]
    fn routing_eligible_providers_includes_copilot_when_no_keys() {
        let partial = PartialConfig::default();
        let eligible = eligible_routing_providers(&partial, &ProvidersConfig::default());
        assert_eq!(eligible, vec![ProviderId::Copilot]);
    }

    #[test]
    fn routing_eligible_providers_appends_copilot_after_effective_keyed_providers() {
        let partial = PartialConfig::default();
        let providers = ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("sk-test")),
                ..Default::default()
            },
            ..Default::default()
        };
        let eligible = eligible_routing_providers(&partial, &providers);
        assert_eq!(eligible, vec![ProviderId::OpenAI, ProviderId::Copilot]);
    }

    #[test]
    fn validate_step3_result_passes_with_copilot_only_flag() {
        let partial = PartialConfig::default();
        assert!(
            validate_step3_result(&partial, &ProvidersConfig::default(), false, false).is_err()
        );
        assert!(validate_step3_result(&partial, &ProvidersConfig::default(), true, false).is_ok());
    }

    #[test]
    fn should_offer_copilot_only_bypass_returns_false_when_effective_env_key_exists() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();

        unsafe {
            std::env::set_var("SCORPIO_OPENAI_API_KEY", "test-key");
        }

        let partial = PartialConfig::default();
        let providers =
            scorpio_core::config::Config::load_effective_providers_config_from_user_path(
                &config_path,
                &partial,
            )
            .unwrap_or_default();

        assert!(!providers_with_keys(&partial, &providers).is_empty());

        unsafe {
            std::env::remove_var("SCORPIO_OPENAI_API_KEY");
        }
    }

    #[test]
    fn default_routing_from_step3_copilot_only_preselects_both_tiers() {
        let defaults = default_routing_from_step3(&StepThreeOutcome { copilot_only: true });

        assert_eq!(defaults.quick_provider, Some(ProviderId::Copilot));
        assert_eq!(defaults.deep_provider, Some(ProviderId::Copilot));
        assert!(defaults.keyed_providers_skipped_message);
    }

    #[test]
    fn provider_key_copilot_always_returns_none() {
        let partial = PartialConfig::default();
        assert_eq!(provider_key(&partial, ProviderId::Copilot), None);
    }

    #[test]
    fn provider_key_and_set_provider_key_handle_xiaomimimo() {
        let mut partial = PartialConfig::default();
        set_provider_key(
            &mut partial,
            ProviderId::XiaomiMimo,
            Some("mimo-key".into()),
        );
        assert_eq!(
            provider_key(&partial, ProviderId::XiaomiMimo),
            Some("mimo-key")
        );
    }

    #[test]
    fn providers_with_keys_includes_xiaomimimo() {
        let p = PartialConfig {
            xiaomimimo_api_key: Some("m".into()),
            ..Default::default()
        };
        assert_eq!(
            providers_with_keys(&p, &ProvidersConfig::default()),
            vec![ProviderId::XiaomiMimo]
        );
    }

    #[test]
    fn providers_with_keys_picks_up_key_from_effective_providers() {
        let partial = PartialConfig::default();
        let providers = ProvidersConfig {
            anthropic: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("sk-ant")),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            providers_with_keys(&partial, &providers),
            vec![ProviderId::Anthropic]
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
    fn load_effective_runtime_preserves_provider_overrides_from_partial() {
        let partial = PartialConfig {
            quick_thinking_provider: Some("openai".into()),
            quick_thinking_model: Some("gpt-4o-mini".into()),
            deep_thinking_provider: Some("openai".into()),
            deep_thinking_model: Some("o3".into()),
            openai_api_key: Some("sk-from-file".into()),
            openai_base_url: Some("https://openai.example.com/v1".into()),
            openai_rpm: Some(123),
            ..Default::default()
        };

        let cfg = scorpio_core::config::Config::load_effective_runtime(partial)
            .expect("merged config should load");

        assert_eq!(
            cfg.providers.openai.base_url.as_deref(),
            Some("https://openai.example.com/v1")
        );
        assert_eq!(cfg.providers.openai.rpm, 123);
    }

    // ── normalize_optional_account (Futu) ─────────────────────────────────────

    #[test]
    fn normalize_optional_account_blank_is_auto_select() {
        assert_eq!(normalize_optional_account(""), None);
        assert_eq!(normalize_optional_account("   "), None);
    }

    #[test]
    fn normalize_optional_account_trims_and_keeps_any_identifier() {
        // Universal account number, card number, or raw acc_id — all kept verbatim.
        assert_eq!(
            normalize_optional_account("  1001100580092142 "),
            Some("1001100580092142".to_owned())
        );
        assert_eq!(
            normalize_optional_account("281756460288629917"),
            Some("281756460288629917".to_owned())
        );
    }

    // ── ProviderId Display ────────────────────────────────────────────────────

    #[test]
    fn provider_id_display_matches_as_str() {
        for &p in KEYED_WIZARD_PROVIDERS {
            assert_eq!(p.to_string(), p.as_str());
        }
    }
}
