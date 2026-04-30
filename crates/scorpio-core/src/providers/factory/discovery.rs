use std::collections::HashMap;
use std::time::Duration;

use futures::future::join_all;
use rig::client::ModelListingClient;
use rig::model::ModelList;
use tokio::time::timeout;

use crate::config::ProvidersConfig;
use crate::providers::ProviderId;

use super::error::sanitize_error_summary;

const DISCOVERY_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Result of setup-time model discovery for a provider.
///
/// This enum is only for the interactive setup flow. It decides whether setup
/// can show a picker of discovered model IDs, must fall back to manual entry,
/// or should explain that listing is temporarily unavailable.
pub enum ModelDiscoveryOutcome {
    Listed(Vec<String>),
    ManualOnly { reason: String },
    Unavailable { reason: String },
}

/// Discover setup-time model options for the given eligible providers.
///
/// This helper is intentionally setup-only: it returns prompt-oriented outcomes
/// for the wizard and is not a general runtime provider-readiness check.
pub async fn discover_setup_models(
    eligible: &[ProviderId],
    providers: &ProvidersConfig,
) -> HashMap<ProviderId, ModelDiscoveryOutcome> {
    discover_setup_models_with(eligible.iter().copied(), |provider| async move {
        match provider {
            ProviderId::OpenRouter => Err("manual-only".to_owned()),
            ProviderId::OpenAI => list_openai_models(&providers.openai).await,
            ProviderId::Anthropic => list_anthropic_models(&providers.anthropic).await,
            ProviderId::Gemini => list_gemini_models(&providers.gemini).await,
            ProviderId::DeepSeek => list_deepseek_models(&providers.deepseek).await,
        }
    })
    .await
}

async fn discover_setup_models_with<I, F, Fut>(
    eligible: I,
    load: F,
) -> HashMap<ProviderId, ModelDiscoveryOutcome>
where
    I: IntoIterator<Item = ProviderId>,
    F: Fn(ProviderId) -> Fut + Copy,
    Fut: Future<Output = Result<ModelList, String>>,
{
    collect_discovery_outcomes(eligible.into_iter(), |provider| async move {
        let outcome = match provider {
            ProviderId::OpenRouter => manual_only_outcome(provider),
            _ => match timeout(Duration::from_secs(DISCOVERY_TIMEOUT_SECS), load(provider)).await {
                Ok(Ok(models)) => normalize_model_list(provider, models),
                Ok(Err(err)) => unavailable_from_error(provider, &err),
                Err(_elapsed) => ModelDiscoveryOutcome::Unavailable {
                    reason: format!(
                        "Listing for {} timed out; enter the model manually.",
                        provider.as_str()
                    ),
                },
            },
        };
        (provider, outcome)
    })
    .await
}

async fn collect_discovery_outcomes<I, F, Fut>(
    eligible: I,
    to_outcome: F,
) -> HashMap<ProviderId, ModelDiscoveryOutcome>
where
    I: IntoIterator<Item = ProviderId>,
    F: Fn(ProviderId) -> Fut + Copy,
    Fut: Future<Output = (ProviderId, ModelDiscoveryOutcome)>,
{
    let futures: Vec<_> = eligible.into_iter().map(to_outcome).collect();
    let results = join_all(futures).await;
    results.into_iter().collect()
}

fn manual_only_outcome(provider: ProviderId) -> ModelDiscoveryOutcome {
    ModelDiscoveryOutcome::ManualOnly {
        reason: format!(
            "Model listing is manual-only for {}; enter the model manually.",
            provider.as_str()
        ),
    }
}

fn normalize_model_list(provider: ProviderId, list: ModelList) -> ModelDiscoveryOutcome {
    if list.is_empty() {
        return ModelDiscoveryOutcome::Unavailable {
            reason: format!(
                "No models were returned for {}; enter the model manually.",
                provider.as_str()
            ),
        };
    }
    let ids: Vec<String> = list.into_iter().map(|m| m.id).collect();
    ModelDiscoveryOutcome::Listed(ids)
}

fn unavailable_from_error(provider: ProviderId, error: &str) -> ModelDiscoveryOutcome {
    tracing::warn!(
        provider = provider.as_str(),
        error = %sanitize_error_summary(error),
        "list_models failed"
    );
    ModelDiscoveryOutcome::Unavailable {
        reason: format!(
            "Could not load models for {}; enter the model manually.",
            provider.as_str()
        ),
    }
}

// ── Provider-specific model listing helpers ─────────────────────────────────

use crate::config::ProviderSettings;
use secrecy::ExposeSecret;

async fn list_openai_models(settings: &ProviderSettings) -> Result<ModelList, String> {
    if settings.base_url.is_some() {
        return Err("custom base_url requires manual entry".to_owned());
    }
    let key = settings
        .api_key
        .as_ref()
        .ok_or_else(|| "missing API key".to_owned())?;
    let client = rig::providers::openai::Client::new(key.expose_secret())
        .map_err(|e| format!("client build error: {e}"))?;
    client.list_models().await.map_err(|e| e.to_string())
}

async fn list_anthropic_models(settings: &ProviderSettings) -> Result<ModelList, String> {
    if settings.base_url.is_some() {
        return Err("custom base_url requires manual entry".to_owned());
    }
    let key = settings
        .api_key
        .as_ref()
        .ok_or_else(|| "missing API key".to_owned())?;
    let client = rig::providers::anthropic::Client::new(key.expose_secret())
        .map_err(|e| format!("client build error: {e}"))?;
    client.list_models().await.map_err(|e| e.to_string())
}

async fn list_gemini_models(settings: &ProviderSettings) -> Result<ModelList, String> {
    if settings.base_url.is_some() {
        return Err("custom base_url requires manual entry".to_owned());
    }
    let key = settings
        .api_key
        .as_ref()
        .ok_or_else(|| "missing API key".to_owned())?;
    let client = rig::providers::gemini::Client::new(key.expose_secret())
        .map_err(|e| format!("client build error: {e}"))?;
    client.list_models().await.map_err(|e| e.to_string())
}

async fn list_deepseek_models(settings: &ProviderSettings) -> Result<ModelList, String> {
    if settings.base_url.is_some() {
        return Err("custom base_url requires manual entry".to_owned());
    }
    let key = settings
        .api_key
        .as_ref()
        .ok_or_else(|| "missing API key".to_owned())?;
    let client = rig::providers::deepseek::Client::new(key.expose_secret())
        .map_err(|e| format!("client build error: {e}"))?;
    client.list_models().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::model::{Model, ModelList};

    #[test]
    fn openrouter_returns_manual_only() {
        let outcome = manual_only_outcome(ProviderId::OpenRouter);
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::ManualOnly {
                reason: "Model listing is manual-only for openrouter; enter the model manually."
                    .into(),
            }
        );
    }

    #[test]
    fn normalize_model_list_preserves_order_and_duplicates() {
        let list = ModelList::new(vec![
            Model::from_id("gpt-4o-mini"),
            Model::from_id("o3"),
            Model::from_id("o3"),
        ]);
        let outcome = normalize_model_list(ProviderId::OpenAI, list);
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into(), "o3".into(), "o3".into(),])
        );
    }

    #[test]
    fn normalize_empty_model_list_returns_unavailable() {
        let outcome = normalize_model_list(ProviderId::Gemini, ModelList::new(vec![]));
        assert_eq!(
            outcome,
            ModelDiscoveryOutcome::Unavailable {
                reason: "No models were returned for gemini; enter the model manually.".into(),
            }
        );
    }

    #[test]
    fn unavailable_reason_is_sanitized() {
        let outcome = unavailable_from_error(
            ProviderId::Anthropic,
            "Bearer sk-ant-secret-token leaked from upstream",
        );
        let ModelDiscoveryOutcome::Unavailable { reason } = outcome else {
            panic!("expected unavailable outcome");
        };
        assert!(reason.contains("anthropic"));
        assert!(!reason.contains("sk-ant-secret-token"));
    }

    #[tokio::test]
    async fn collect_outcomes_keeps_one_result_per_provider() {
        let outcomes = collect_discovery_outcomes(
            [ProviderId::OpenAI, ProviderId::OpenRouter],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => (
                        provider,
                        ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]),
                    ),
                    ProviderId::OpenRouter => (provider, manual_only_outcome(provider)),
                    _ => unreachable!(),
                }
            },
        )
        .await;
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(
            outcomes.get(&ProviderId::OpenAI),
            Some(ModelDiscoveryOutcome::Listed(_))
        ));
        assert!(matches!(
            outcomes.get(&ProviderId::OpenRouter),
            Some(ModelDiscoveryOutcome::ManualOnly { .. })
        ));
    }

    #[tokio::test]
    async fn discover_setup_models_with_sanitizes_failures_and_preserves_successes() {
        let outcomes = discover_setup_models_with(
            [ProviderId::OpenAI, ProviderId::Anthropic],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => Ok(ModelList::new(vec![Model::from_id("gpt-4o-mini")])),
                    ProviderId::Anthropic => {
                        Err("Bearer sk-ant-secret-token leaked from upstream".to_owned())
                    }
                    _ => unreachable!(),
                }
            },
        )
        .await;
        assert_eq!(
            outcomes.get(&ProviderId::OpenAI),
            Some(&ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]))
        );
        assert_eq!(
            outcomes.get(&ProviderId::Anthropic),
            Some(&ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for anthropic; enter the model manually.".into(),
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn discover_setup_models_with_times_out_slow_providers_without_blocking_others() {
        let outcomes = discover_setup_models_with(
            [ProviderId::OpenAI, ProviderId::Anthropic],
            |provider| async move {
                match provider {
                    ProviderId::OpenAI => Ok(ModelList::new(vec![Model::from_id("gpt-4o-mini")])),
                    ProviderId::Anthropic => {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        Ok(ModelList::new(vec![Model::from_id("claude-haiku")]))
                    }
                    _ => unreachable!(),
                }
            },
        )
        .await;
        assert!(matches!(
            outcomes.get(&ProviderId::OpenAI),
            Some(ModelDiscoveryOutcome::Listed(_))
        ));
        let anthropic = outcomes.get(&ProviderId::Anthropic);
        let Some(ModelDiscoveryOutcome::Unavailable { reason }) = anthropic else {
            panic!("expected Unavailable for slow provider; got {anthropic:?}");
        };
        assert!(reason.contains("anthropic"));
        assert!(reason.contains("timed out") || reason.contains("Could not load"));
    }

    #[test]
    fn unavailable_reason_uses_fixed_template_regardless_of_upstream_error_shape() {
        let leak_patterns = [
            "Bearer sk-ant-secret-token leaked from upstream",
            "x-api-key: sk-real-key was rejected",
            "Authorization: Bearer sk-secret-key invalid",
            "request to https://api.example.com?api_key=sk-leaked failed",
            "raw sk-rawtoken at the start of the message",
            "{\"error\":{\"message\":\"Invalid Authorization: Bearer sk-leaked\"}}",
        ];
        for upstream in leak_patterns {
            let outcome = unavailable_from_error(ProviderId::OpenAI, upstream);
            let ModelDiscoveryOutcome::Unavailable { reason } = outcome else {
                panic!("expected unavailable outcome for upstream={upstream:?}");
            };
            assert_eq!(
                reason, "Could not load models for openai; enter the model manually.",
                "reason must come from a fixed template; got {reason:?} for upstream={upstream:?}"
            );
            assert!(
                reason.len() <= 120,
                "reason exceeds 120-char cap: {reason:?}"
            );
        }
    }

    #[tokio::test]
    async fn discover_setup_models_treats_custom_base_url_as_unavailable() {
        let providers = ProvidersConfig {
            openai: ProviderSettings {
                api_key: Some(secrecy::SecretString::from("sk-test")),
                base_url: Some("https://gateway.internal/v1".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let outcomes = discover_setup_models(&[ProviderId::OpenAI], &providers).await;

        assert_eq!(
            outcomes.get(&ProviderId::OpenAI),
            Some(&ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for openai; enter the model manually.".into(),
            })
        );
    }
}
