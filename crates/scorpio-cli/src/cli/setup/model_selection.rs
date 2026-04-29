use std::collections::HashMap;
use std::path::Path;

use scorpio_core::config::Config;
use scorpio_core::providers::ProviderId;
use scorpio_core::providers::factory::{ModelDiscoveryOutcome, discover_setup_models};
use scorpio_core::settings::PartialConfig;

use super::steps::apply_provider_routing;

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelMenuOption {
    Listed(String),
    Manual,
}

impl std::fmt::Display for ModelMenuOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listed(name) => f.write_str(name),
            Self::Manual => f.write_str("Enter model manually"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelPromptMode {
    Select {
        options: Vec<ModelMenuOption>,
        default_index: usize,
    },
    Manual {
        note: Option<String>,
        initial_value: String,
    },
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct TierRoutingPlan {
    provider: ProviderId,
    prompt_mode: ModelPromptMode,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderRoutingPlan {
    quick: TierRoutingPlan,
    deep: TierRoutingPlan,
}

fn default_provider_index(eligible: &[ProviderId], saved_provider: Option<&str>) -> usize {
    saved_provider
        .and_then(|name| eligible.iter().position(|p| p.as_str() == name))
        .unwrap_or(0)
}

fn listed_model_options(models: &[String], saved_model: Option<&str>) -> Vec<ModelMenuOption> {
    let mut result: Vec<ModelMenuOption> = Vec::with_capacity(models.len() + 1);

    if let Some(saved) = saved_model {
        if let Some(pos) = models.iter().position(|m| m == saved) {
            result.push(ModelMenuOption::Listed(saved.to_owned()));
            for (i, model) in models.iter().enumerate() {
                if i != pos {
                    result.push(ModelMenuOption::Listed(model.clone()));
                }
            }
        } else {
            for model in models {
                result.push(ModelMenuOption::Listed(model.clone()));
            }
        }
    } else {
        for model in models {
            result.push(ModelMenuOption::Listed(model.clone()));
        }
    }

    result.push(ModelMenuOption::Manual);
    result
}

fn prompt_mode_for_provider(
    provider: ProviderId,
    outcome: &ModelDiscoveryOutcome,
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> ModelPromptMode {
    let provider_matches =
        saved_provider.is_some_and(|sp| sp.eq_ignore_ascii_case(provider.as_str()));
    let effective_saved_model = if provider_matches { saved_model } else { None };

    match outcome {
        ModelDiscoveryOutcome::Listed(models) => {
            let options = listed_model_options(models, effective_saved_model);
            let default_index = if effective_saved_model.is_some()
                && models
                    .iter()
                    .any(|m| Some(m.as_str()) == effective_saved_model)
            {
                0
            } else {
                options.len() - 1
            };
            ModelPromptMode::Select {
                options,
                default_index,
            }
        }
        ModelDiscoveryOutcome::ManualOnly { reason } => ModelPromptMode::Manual {
            note: Some(reason.clone()),
            initial_value: manual_initial_value(provider, saved_provider, saved_model),
        },
        ModelDiscoveryOutcome::Unavailable { reason } => ModelPromptMode::Manual {
            note: Some(reason.clone()),
            initial_value: manual_initial_value(provider, saved_provider, saved_model),
        },
    }
}

fn manual_initial_value(
    provider: ProviderId,
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> String {
    let provider_matches =
        saved_provider.is_some_and(|sp| sp.eq_ignore_ascii_case(provider.as_str()));
    if provider_matches {
        saved_model.unwrap_or("").to_owned()
    } else {
        String::new()
    }
}

fn prompt_provider(
    label: &str,
    eligible: &[ProviderId],
    saved_provider: Option<&str>,
) -> Result<ProviderId, inquire::InquireError> {
    let default_idx = default_provider_index(eligible, saved_provider);
    inquire::Select::new(label, eligible.to_vec())
        .with_starting_cursor(default_idx)
        .prompt()
}

fn prompt_model_for_provider(
    provider: ProviderId,
    outcome: &ModelDiscoveryOutcome,
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> Result<String, inquire::InquireError> {
    let mode = prompt_mode_for_provider(provider, outcome, saved_provider, saved_model);
    match mode {
        ModelPromptMode::Select {
            options,
            default_index,
        } => {
            let choice = inquire::Select::new(&format!("{provider} model:"), options.clone())
                .with_starting_cursor(default_index)
                .prompt()?;

            match choice {
                ModelMenuOption::Listed(name) => Ok(name),
                ModelMenuOption::Manual => {
                    let initial = manual_initial_value(provider, saved_provider, saved_model);
                    prompt_manual_model(&format!("{provider} model:"), &initial)
                }
            }
        }
        ModelPromptMode::Manual {
            note,
            initial_value,
        } => {
            if let Some(msg) = note {
                println!("{msg}");
            }
            prompt_manual_model(&format!("{provider} model:"), &initial_value)
        }
    }
}

fn prompt_manual_model(label: &str, initial: &str) -> Result<String, inquire::InquireError> {
    inquire::Text::new(label)
        .with_initial_value(initial)
        .with_validator(|s: &str| {
            if s.trim().is_empty() {
                Ok(inquire::validator::Validation::Invalid(
                    "Model name must not be empty".into(),
                ))
            } else {
                Ok(inquire::validator::Validation::Valid)
            }
        })
        .prompt()
}

fn discover_setup_models_blocking(
    config_path: &Path,
    partial: &PartialConfig,
    eligible: &[ProviderId],
) -> HashMap<ProviderId, ModelDiscoveryOutcome> {
    let providers =
        match Config::load_effective_providers_config_from_user_path(config_path, partial) {
            Ok(p) => p,
            Err(_) => {
                return eligible
                    .iter()
                    .map(|&p| {
                        (
                            p,
                            ModelDiscoveryOutcome::Unavailable {
                                reason: format!(
                                    "Could not load models for {}; enter the model manually.",
                                    p.as_str()
                                ),
                            },
                        )
                    })
                    .collect();
            }
        };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => {
            return eligible
                .iter()
                .map(|&p| {
                    (
                        p,
                        ModelDiscoveryOutcome::Unavailable {
                            reason: format!(
                                "Could not load models for {}; enter the model manually.",
                                p.as_str()
                            ),
                        },
                    )
                })
                .collect();
        }
    };

    runtime.block_on(discover_setup_models(eligible, &providers))
}

#[cfg(test)]
fn build_provider_routing_plan(
    eligible: &[ProviderId],
    discovery: &HashMap<ProviderId, ModelDiscoveryOutcome>,
    saved_quick_provider: Option<&str>,
    saved_quick_model: Option<&str>,
    saved_deep_provider: Option<&str>,
    saved_deep_model: Option<&str>,
) -> ProviderRoutingPlan {
    let quick_idx = default_provider_index(eligible, saved_quick_provider);
    let quick_provider = eligible[quick_idx];
    let quick_fallback = ModelDiscoveryOutcome::Unavailable {
        reason: format!(
            "Could not load models for {}; enter the model manually.",
            quick_provider.as_str()
        ),
    };
    let quick_outcome = discovery.get(&quick_provider).unwrap_or(&quick_fallback);
    let quick_mode = prompt_mode_for_provider(
        quick_provider,
        quick_outcome,
        saved_quick_provider,
        saved_quick_model,
    );

    let deep_idx = default_provider_index(eligible, saved_deep_provider);
    let deep_provider = eligible[deep_idx];
    let deep_fallback = ModelDiscoveryOutcome::Unavailable {
        reason: format!(
            "Could not load models for {}; enter the model manually.",
            deep_provider.as_str()
        ),
    };
    let deep_outcome = discovery.get(&deep_provider).unwrap_or(&deep_fallback);
    let deep_mode = prompt_mode_for_provider(
        deep_provider,
        deep_outcome,
        saved_deep_provider,
        saved_deep_model,
    );

    ProviderRoutingPlan {
        quick: TierRoutingPlan {
            provider: quick_provider,
            prompt_mode: quick_mode,
        },
        deep: TierRoutingPlan {
            provider: deep_provider,
            prompt_mode: deep_mode,
        },
    }
}

pub fn prompt_provider_routing(
    partial: &mut PartialConfig,
    eligible: Vec<ProviderId>,
    config_path: &Path,
) -> Result<(), inquire::InquireError> {
    let discovery = discover_setup_models_blocking(config_path, partial, &eligible);

    let quick_provider = prompt_provider(
        "Quick-thinking provider (used by analyst agents):",
        &eligible,
        partial.quick_thinking_provider.as_deref(),
    )?;
    let quick_model = prompt_model_for_provider(
        quick_provider,
        discovery
            .get(&quick_provider)
            .expect("provider outcome exists"),
        partial.quick_thinking_provider.as_deref(),
        partial.quick_thinking_model.as_deref(),
    )?;

    let deep_provider = prompt_provider(
        "Deep-thinking provider (used by researcher, trader, and risk agents):",
        &eligible,
        partial.deep_thinking_provider.as_deref(),
    )?;
    let deep_model = prompt_model_for_provider(
        deep_provider,
        discovery
            .get(&deep_provider)
            .expect("provider outcome exists"),
        partial.deep_thinking_provider.as_deref(),
        partial.deep_thinking_model.as_deref(),
    )?;

    apply_provider_routing(
        partial,
        (quick_provider, quick_model),
        (deep_provider, deep_model),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_core::providers::ProviderId;
    use scorpio_core::providers::factory::ModelDiscoveryOutcome;

    #[test]
    fn default_provider_index_falls_back_to_first_eligible_when_saved_provider_is_unsupported() {
        let eligible = vec![
            ProviderId::OpenAI,
            ProviderId::Anthropic,
            ProviderId::DeepSeek,
        ];
        assert_eq!(default_provider_index(&eligible, Some("copilot")), 0);
    }

    #[test]
    fn listed_model_options_put_saved_model_first_and_manual_last() {
        let options = listed_model_options(
            &["gpt-4o-mini".into(), "o3".into(), "gpt-4o-mini".into()],
            Some("o3"),
        );
        assert_eq!(
            options,
            vec![
                ModelMenuOption::Listed("o3".into()),
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Manual,
            ]
        );
    }

    #[test]
    fn listed_model_options_keep_provider_order_when_saved_model_missing() {
        let options =
            listed_model_options(&["gpt-4o-mini".into(), "o3".into()], Some("claude-opus"));
        assert_eq!(
            options,
            vec![
                ModelMenuOption::Listed("gpt-4o-mini".into()),
                ModelMenuOption::Listed("o3".into()),
                ModelMenuOption::Manual,
            ]
        );
    }

    #[test]
    fn prompt_mode_defaults_picker_to_manual_when_saved_model_not_listed() {
        let mode = prompt_mode_for_provider(
            ProviderId::OpenAI,
            &ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]),
            Some("openai"),
            Some("o3"),
        );
        assert_eq!(
            mode,
            ModelPromptMode::Select {
                options: vec![
                    ModelMenuOption::Listed("gpt-4o-mini".into()),
                    ModelMenuOption::Manual,
                ],
                default_index: 1,
            }
        );
    }

    #[test]
    fn manual_prefill_uses_saved_model_only_after_manual_option_is_selected() {
        let initial = manual_initial_value(ProviderId::OpenAI, Some("openai"), Some("o3"));
        assert_eq!(initial, "o3");
    }

    #[test]
    fn prompt_mode_uses_unavailable_note_for_failed_listing() {
        let mode = prompt_mode_for_provider(
            ProviderId::Gemini,
            &ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for gemini; enter the model manually.".into(),
            },
            Some("gemini"),
            Some("gemini-2.5-pro"),
        );
        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some("Could not load models for gemini; enter the model manually.".into()),
                initial_value: "gemini-2.5-pro".into(),
            }
        );
    }

    #[test]
    fn prompt_mode_manual_only_skips_picker_and_goes_straight_to_text_entry() {
        let mode = prompt_mode_for_provider(
            ProviderId::OpenRouter,
            &ModelDiscoveryOutcome::ManualOnly {
                reason: "Model listing is manual-only for openrouter; enter the model manually."
                    .into(),
            },
            Some("openrouter"),
            Some("qwen/qwen3.6-plus-preview:free"),
        );
        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some(
                    "Model listing is manual-only for openrouter; enter the model manually.".into()
                ),
                initial_value: "qwen/qwen3.6-plus-preview:free".into(),
            }
        );
    }

    #[test]
    fn prompt_mode_does_not_prefill_saved_model_when_saved_provider_differs() {
        let mode = prompt_mode_for_provider(
            ProviderId::Anthropic,
            &ModelDiscoveryOutcome::Unavailable {
                reason: "Could not load models for anthropic; enter the model manually.".into(),
            },
            Some("openai"),
            Some("gpt-4o-mini"),
        );
        assert_eq!(
            mode,
            ModelPromptMode::Manual {
                note: Some("Could not load models for anthropic; enter the model manually.".into()),
                initial_value: String::new(),
            }
        );
    }

    #[test]
    fn build_provider_routing_plan_reuses_one_prefetched_snapshot_for_both_tiers() {
        let eligible = vec![ProviderId::OpenAI, ProviderId::DeepSeek];
        let discovery = std::collections::HashMap::from([
            (
                ProviderId::OpenAI,
                ModelDiscoveryOutcome::Listed(vec!["gpt-4o-mini".into()]),
            ),
            (
                ProviderId::DeepSeek,
                ModelDiscoveryOutcome::Listed(vec!["deepseek-chat".into()]),
            ),
        ]);
        let plan = build_provider_routing_plan(
            &eligible,
            &discovery,
            Some("openai"),
            Some("gpt-4o-mini"),
            Some("deepseek"),
            Some("deepseek-chat"),
        );
        assert_eq!(plan.quick.provider, ProviderId::OpenAI);
        assert_eq!(plan.deep.provider, ProviderId::DeepSeek);
        assert!(matches!(
            plan.quick.prompt_mode,
            ModelPromptMode::Select { .. }
        ));
        assert!(matches!(
            plan.deep.prompt_mode,
            ModelPromptMode::Select { .. }
        ));
    }
}
