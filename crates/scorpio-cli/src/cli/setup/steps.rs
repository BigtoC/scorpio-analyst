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

use scorpio_core::config::ProvidersConfig;
use scorpio_core::constants::HEALTH_CHECK_TIMEOUT_SECS;
use scorpio_core::error::RetryPolicy;
use scorpio_core::providers::{ModelTier, ProviderId};
use scorpio_core::settings::PartialConfig;

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

/// Alias kept for call sites that haven't yet migrated to `KEYED_WIZARD_PROVIDERS`.
pub const WIZARD_PROVIDERS: &[ProviderId] = KEYED_WIZARD_PROVIDERS;

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

    loop {
        let mut items: Vec<ProviderSelectItem> = provider_choices(partial)
            .into_iter()
            .map(ProviderSelectItem::Provider)
            .collect();
        items.push(ProviderSelectItem::Skip);

        let selection =
            inquire::Select::new("Select an LLM provider to configure:", items).prompt()?;

        let chosen = match selection {
            ProviderSelectItem::Skip => {
                if validate_step3_result(partial, &effective_providers, false).is_ok() {
                    break;
                }
                println!("✗ At least one LLM provider is required.");
                continue;
            }
            ProviderSelectItem::Provider(c) => c.provider,
        };
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

        if validate_step3_result(partial, &effective_providers, false).is_err() {
            // Haven't met the minimum yet — loop without asking "more?".
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

        if providers_with_keys(partial, &effective_providers).len() == KEYED_WIZARD_PROVIDERS.len()
        {
            break;
        }
    }
    Ok(StepThreeOutcome {
        copilot_only: false,
    })
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

        let auth_completed = run_copilot_auth_loop(
            || run_single_copilot_health_check(&copilot_tiers, &cfg, &rate_limiters, &token_dir),
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

/// Return `Err` when no LLM provider key is present (unless `copilot_only_selected`).
pub(super) fn validate_step3_result(
    partial: &PartialConfig,
    effective_providers: &ProvidersConfig,
    copilot_only_selected: bool,
) -> Result<(), &'static str> {
    if copilot_only_selected {
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

fn describe_health_check_targets(cfg: &scorpio_core::config::Config) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "quick-thinking ({} / {})",
        cfg.llm.quick_thinking_provider, cfg.llm.quick_thinking_model
    ));
    parts.push(format!(
        "deep-thinking ({} / {})",
        cfg.llm.deep_thinking_provider, cfg.llm.deep_thinking_model
    ));
    parts.join(", ")
}

fn run_selected_model_tiers(
    cfg: &scorpio_core::config::Config,
    tiers: &[ModelTier],
) -> anyhow::Result<()> {
    let rate_limiters = scorpio_core::rate_limit::ProviderRateLimiters::from_config(&cfg.providers);
    cfg.is_analysis_ready()
        .context("effective runtime config is not ready for analysis")?;

    check_selected_model_tiers(tiers.iter().copied(), |tier| {
        let handle = scorpio_core::providers::factory::create_completion_model(
            tier,
            &cfg.llm,
            &cfg.providers,
            &rate_limiters,
        )
        .map_err(|e| anyhow::anyhow!("failed to create completion model: {e}"))?;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build runtime for health check")?;

        runtime
            .block_on(async {
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
    })
}

fn effective_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider == "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider == "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

fn configured_non_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider != "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider != "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

fn run_copilot_auth_loop<Run, Report, Retry, Back>(
    mut run_check: Run,
    mut report_failure: Report,
    mut should_retry: Retry,
    mut should_back: Back,
) -> anyhow::Result<bool>
where
    Run: FnMut() -> anyhow::Result<()>,
    Report: FnMut(&anyhow::Error),
    Retry: FnMut() -> anyhow::Result<bool>,
    Back: FnMut() -> anyhow::Result<bool>,
{
    loop {
        match run_check() {
            Ok(()) => return Ok(true),
            Err(error) => {
                report_failure(&error);
                if should_retry()? {
                    continue;
                }
                return should_back();
            }
        }
    }
}

fn run_single_copilot_health_check(
    tiers: &[ModelTier],
    cfg: &scorpio_core::config::Config,
    rate_limiters: &scorpio_core::rate_limit::ProviderRateLimiters,
    token_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for Copilot health check")?;

    let handles: Vec<_> = tiers
        .iter()
        .copied()
        .map(|tier| {
            scorpio_core::providers::factory::create_completion_model_with_copilot(
                tier,
                &cfg.llm,
                &cfg.providers,
                rate_limiters,
                scorpio_core::providers::factory::CopilotAuthMode::InteractiveSetup,
                token_dir,
            )
            .map_err(|e| anyhow::anyhow!("failed to create Copilot completion model: {e}"))
        })
        .collect::<anyhow::Result<_>>()?;

    runtime.block_on(async {
        let first = handles.first().ok_or_else(|| {
            anyhow::anyhow!("Copilot health check requires at least one routed tier")
        })?;

        first
            .authorize_copilot()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        step5_validate_copilot_auth(token_dir).await?;

        for handle in &handles {
            let agent = scorpio_core::providers::factory::build_agent(handle, "");
            scorpio_core::providers::factory::prompt_with_retry(
                &agent,
                "Hello",
                Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS),
                &RetryPolicy::default(),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        }

        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}

async fn step5_validate_copilot_auth(token_dir: &std::path::Path) -> anyhow::Result<()> {
    step5_validate_copilot_auth_with(token_dir, |token| {
        Box::pin(scorpio_core::providers::factory::copilot_auth::fetch_github_identity(token))
    })
    .await
}

async fn step5_validate_copilot_auth_with<F>(
    token_dir: &std::path::Path,
    fetch_identity: F,
) -> anyhow::Result<()>
where
    F: for<'a> Fn(
        &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        scorpio_core::providers::factory::copilot_auth::GitHubIdentity,
                        scorpio_core::error::TradingError,
                    >,
                > + 'a,
        >,
    >,
{
    use scorpio_core::providers::factory::copilot_auth;

    best_effort_harden_copilot_cache_files(token_dir);

    let access =
        copilot_auth::read_access_token(token_dir).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let record =
        copilot_auth::read_api_key_record(token_dir).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    copilot_auth::validate_copilot_runtime_base(&record)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let identity = fetch_identity(&access)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    copilot_auth::validate_scope(&identity.scopes).map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let binding = copilot_auth::ScorpioIdentityBinding {
        github_id: identity.id,
        github_login: identity.login.clone(),
        written_at: chrono::Utc::now().timestamp(),
    };
    copilot_auth::write_binding(token_dir, &binding)?;
    best_effort_harden_copilot_cache_files(token_dir);
    eprintln!(
        "✓ Copilot authorization validated for GitHub login {} and wrote scorpio-identity.json",
        identity.login
    );
    Ok(())
}

fn best_effort_harden_copilot_cache_files(token_dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for name in ["access-token", "api-key.json"] {
            let path = token_dir.join(name);
            if !path.exists() {
                continue;
            }
            if let Err(error) =
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "failed to tighten Copilot cache file permissions"
                );
            }
        }
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

#[derive(Clone, Debug)]
enum ProviderSelectItem {
    Provider(ProviderChoice),
    Skip,
}

impl std::fmt::Display for ProviderSelectItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provider(c) => c.fmt(f),
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
                false
            )
            .is_err()
        );
    }

    #[test]
    fn validate_step3_result_openai_key_returns_ok() {
        assert!(
            validate_step3_result(&partial_with_openai(), &ProvidersConfig::default(), false)
                .is_ok()
        );
    }

    #[test]
    fn validate_step3_result_anthropic_key_returns_ok() {
        let p = PartialConfig {
            anthropic_api_key: Some("sk-ant".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false).is_ok());
    }

    #[test]
    fn validate_step3_result_gemini_key_returns_ok() {
        let p = PartialConfig {
            gemini_api_key: Some("AIza".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false).is_ok());
    }

    #[test]
    fn validate_step3_result_openrouter_key_returns_ok() {
        let p = PartialConfig {
            openrouter_api_key: Some("or-key".into()),
            ..Default::default()
        };
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false).is_ok());
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
        assert!(validate_step3_result(&p, &ProvidersConfig::default(), false).is_ok());
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
        assert!(validate_step3_result(&partial, &ProvidersConfig::default(), false).is_err());
        assert!(validate_step3_result(&partial, &ProvidersConfig::default(), true).is_ok());
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

    #[test]
    fn run_copilot_auth_loop_returns_false_when_retry_and_back_are_declined() {
        let should_continue = run_copilot_auth_loop(
            || anyhow::bail!("persistent failure"),
            |_err| {},
            || Ok(false),
            || Ok(false),
        )
        .expect("declining retry/back should not error");

        assert!(!should_continue);
    }

    #[tokio::test]
    async fn step5_validate_copilot_auth_writes_identity_binding_on_success() {
        use scorpio_core::providers::factory::copilot_auth;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("access-token"), "ghu_test_token").unwrap();
        std::fs::write(
            dir.path().join("api-key.json"),
            r#"{"endpoints":{"api":"https://api.githubcopilot.com"}}"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::set_permissions(
                dir.path().join("access-token"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
            std::fs::set_permissions(
                dir.path().join("api-key.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }

        step5_validate_copilot_auth_with(dir.path(), |_token| {
            Box::pin(async {
                Ok(copilot_auth::GitHubIdentity {
                    id: 42,
                    login: "octocat".to_owned(),
                    scopes: vec!["read:user".to_owned()],
                })
            })
        })
        .await
        .unwrap();

        let binding = copilot_auth::read_binding(dir.path()).unwrap();
        assert_eq!(binding.github_id, 42);
        assert_eq!(binding.github_login, "octocat");
    }

    // ── ProviderId Display ────────────────────────────────────────────────────

    #[test]
    fn provider_id_display_matches_as_str() {
        for &p in WIZARD_PROVIDERS {
            assert_eq!(p.to_string(), p.as_str());
        }
    }
}
