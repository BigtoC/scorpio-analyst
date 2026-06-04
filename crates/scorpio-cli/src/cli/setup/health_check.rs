//! Health-check probes used by the setup wizard.
//!
//! Step 5 (`step5_health_check` in [`super::steps`]) and the trailing
//! Langfuse check at the end of `step_langfuse_observability` delegate the
//! actual network calls into this module. Splitting them out keeps the step
//! orchestrators focused on prompt flow and lets the probes / retry loops be
//! reviewed and unit-tested independently.
//!
//! **Testing note:** Functions that hit real LLM providers (e.g.
//! [`run_single_health_check`], [`run_copilot_auth_only`]) are exercised via
//! manual QA only. The unit tests below cover the generic retry loops,
//! tier-iteration helper, and the Copilot identity-binding write path with an
//! injected `fetch_identity` closure.

use std::time::Duration;

use anyhow::Context;

use scorpio_core::constants::HEALTH_CHECK_TIMEOUT_SECS;
use scorpio_core::error::RetryPolicy;
use scorpio_core::providers::ModelTier;
use scorpio_core::settings::PartialConfig;

// ── Generic retry loops ───────────────────────────────────────────────────────

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

pub(super) fn run_copilot_auth_loop<Run, Report, Retry, Back>(
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

pub(super) fn check_selected_model_tiers<I, Check>(tiers: I, mut check: Check) -> anyhow::Result<()>
where
    I: IntoIterator<Item = ModelTier>,
    Check: FnMut(ModelTier) -> anyhow::Result<()>,
{
    for tier in tiers {
        check(tier).with_context(|| format!("{tier} model health check failed"))?;
    }

    Ok(())
}

// ── LLM tier inspection ───────────────────────────────────────────────────────

pub(super) fn describe_health_check_targets(cfg: &scorpio_core::config::Config) -> String {
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

pub(super) fn effective_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider == "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider == "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

pub(super) fn configured_non_copilot_tiers(cfg: &scorpio_core::config::Config) -> Vec<ModelTier> {
    let mut tiers = Vec::new();
    if cfg.llm.quick_thinking_provider != "copilot" {
        tiers.push(ModelTier::QuickThinking);
    }
    if cfg.llm.deep_thinking_provider != "copilot" {
        tiers.push(ModelTier::DeepThinking);
    }
    tiers
}

// ── LLM probes (non-Copilot) ──────────────────────────────────────────────────

pub(super) fn run_selected_model_tiers(
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
                let timeout = Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS);
                scorpio_core::providers::factory::retry_prompt_budget_loop(
                    &agent,
                    timeout,
                    RetryPolicy::default().total_budget(timeout),
                    &RetryPolicy::default(),
                    || agent.prompt_details("Hello"),
                )
                .await
            })
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!(e))
    })
}

pub(super) fn run_single_health_check(cfg: &scorpio_core::config::Config) -> anyhow::Result<()> {
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
                    let timeout = Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS);
                    scorpio_core::providers::factory::retry_prompt_budget_loop(
                        &agent,
                        timeout,
                        RetryPolicy::default().total_budget(timeout),
                        &RetryPolicy::default(),
                        || agent.prompt_details("Hello"),
                    )
                    .await
                })
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!(e))
        },
    )
}

// ── Copilot auth + probe ──────────────────────────────────────────────────────

/// Phase A of Copilot setup: OAuth grant + identity validation only, no LLM call.
pub(super) fn run_copilot_auth_only(
    tiers: &[ModelTier],
    cfg: &scorpio_core::config::Config,
    rate_limiters: &scorpio_core::rate_limit::ProviderRateLimiters,
    token_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for Copilot auth")?;

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
        let first = handles
            .first()
            .ok_or_else(|| anyhow::anyhow!("Copilot auth requires at least one routed tier"))?;
        first
            .authorize_copilot()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        step5_validate_copilot_auth(token_dir).await
    })
}

/// Phase B of Copilot setup: send a "Hello" probe through each configured Copilot tier.
///
/// Called after [`run_copilot_auth_only`] succeeds. A 400 with
/// `unsupported_api_for_model` means the chosen model requires a different API
/// endpoint (e.g. Responses API) — this surfaces as a model probe failure
/// (with "Save config anyway?") rather than an auth failure.
pub(super) fn run_copilot_model_probe(
    tiers: &[ModelTier],
    cfg: &scorpio_core::config::Config,
    rate_limiters: &scorpio_core::rate_limit::ProviderRateLimiters,
    token_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for Copilot model probe")?;

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
        for handle in &handles {
            let agent = scorpio_core::providers::factory::build_agent(handle, "");
            let timeout = Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS);
            scorpio_core::providers::factory::retry_prompt_budget_loop(
                &agent,
                timeout,
                RetryPolicy::default().total_budget(timeout),
                &RetryPolicy::default(),
                || agent.prompt_details("Hello"),
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        }
        Ok::<(), anyhow::Error>(())
    })
}

pub(super) async fn step5_validate_copilot_auth(token_dir: &std::path::Path) -> anyhow::Result<()> {
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

pub(super) fn best_effort_harden_copilot_cache_files(token_dir: &std::path::Path) {
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

// ── Langfuse health check ─────────────────────────────────────────────────────

/// Probe Langfuse with the just-entered credentials and print the outcome.
///
/// Runs only when public key, secret key, and base URL are all set — the
/// three values are required together to enable OTel export. Failures are
/// reported but never propagated: Langfuse is optional and a probe failure
/// must not block the wizard from completing.
pub(super) fn report_langfuse_health_check(partial: &PartialConfig) {
    match (
        partial.langfuse_public_key.as_deref(),
        partial.langfuse_secret_key.as_deref(),
        partial.langfuse_base_url.as_deref(),
    ) {
        (Some(public_key), Some(secret_key), Some(base_url)) => {
            println!("Checking Langfuse connectivity...");
            match run_langfuse_health_check(public_key, secret_key, base_url) {
                Ok(()) => println!("✓ Langfuse health check passed."),
                Err(e) => eprintln!("✗ Langfuse health check failed: {e:#}"),
            }
        }
        (None, None, None) => {}
        _ => {
            println!(
                "Note: Langfuse export needs all three of public key, secret key, and base URL — health check skipped."
            );
        }
    }
}

/// Issue an authenticated `GET <base_url>/api/public/projects` to verify the
/// credentials. A 200 response confirms both connectivity and that the
/// public/secret pair belong to a real Langfuse project.
fn run_langfuse_health_check(
    public_key: &str,
    secret_key: &str,
    base_url: &str,
) -> anyhow::Result<()> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/public/projects");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for Langfuse health check")?;

    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS))
            .build()
            .context("failed to build HTTP client")?;

        let response = client
            .get(&url)
            .basic_auth(public_key, Some(secret_key))
            .send()
            .await
            .with_context(|| format!("could not reach Langfuse at {base}"))?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            anyhow::bail!("Langfuse rejected credentials (HTTP {status})");
        }
        anyhow::bail!("Langfuse returned HTTP {status}")
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}
