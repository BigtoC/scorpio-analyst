//! Shared helpers for analyst agents.
//!
//! Extracted here to avoid verbatim duplication across the four analyst
//! modules (`fundamental`, `sentiment`, `news`, `technical`).

use std::time::Duration;

use serde::de::DeserializeOwned;

#[cfg(test)]
use crate::agents::shared::agent_token_usage_from_completion;
use crate::{
    agents::shared::sanitize_prompt_context,
    analysis_packs::RuntimePolicy,
    config::LlmConfig,
    constants::MAX_SUMMARY_CHARS,
    error::{RetryPolicy, TradingError},
    providers::{
        ProviderId,
        factory::{LlmAgent, prompt_text_with_retry, prompt_typed_with_retry},
    },
};

/// Render an equity analyst's system prompt from the active pack's slot.
///
/// `base_template` is the role's `prompt_bundle.<analyst>_analyst` text, which
/// preflight's completeness gate has already proven non-empty. The pack has
/// already appended the analyst runtime contract (evidence-discipline rules +
/// unsupported-inference guards) at load time, so this helper is purely a
/// placeholder-substitution step over `{ticker}`, `{current_date}`, and
/// `{analysis_emphasis}`.
pub(super) fn render_analyst_system_prompt(
    base_template: &str,
    symbol: &str,
    target_date: &str,
    policy: &RuntimePolicy,
) -> String {
    let analysis_emphasis = sanitize_prompt_context(&policy.analysis_emphasis);
    base_template
        .replace("{ticker}", symbol)
        .replace("{current_date}", target_date)
        .replace("{analysis_emphasis}", &analysis_emphasis)
}

/// Shared runtime fields derived from the analyst request context.
///
/// Keeping these fields together removes duplicated constructor code while
/// preserving explicit, agent-specific constructors in each analyst module.
pub(super) struct AnalystRuntimeConfig {
    pub symbol: String,
    pub target_date: String,
    pub timeout: Duration,
    pub retry_policy: RetryPolicy,
}

/// Build the common runtime configuration shared by all analyst agents.
pub(super) fn analyst_runtime_config(
    symbol: impl Into<String>,
    target_date: impl Into<String>,
    llm_config: &LlmConfig,
) -> AnalystRuntimeConfig {
    AnalystRuntimeConfig {
        symbol: symbol.into(),
        target_date: target_date.into(),
        timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
        retry_policy: RetryPolicy::from_config(llm_config),
    }
}

/// Validate that a summary is within length bounds and free of control characters.
pub(super) fn validate_summary_content(context: &str, summary: &str) -> Result<(), TradingError> {
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: summary exceeds maximum {MAX_SUMMARY_CHARS} characters"),
        });
    }
    if summary
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: format!("{context}: summary contains disallowed control characters"),
        });
    }
    Ok(())
}

// ─── run_analyst_inference ────────────────────────────────────────────────────

/// Result of a single analyst LLM inference call.
///
/// Bundles the parsed/validated output with usage metadata so callers can
/// forward both pieces to the shared token-usage helper without re-querying the agent.
pub(super) struct AnalystInferenceOutcome<T> {
    /// The successfully parsed and validated output from the LLM.
    pub output: T,
    /// Provider-reported token usage from the underlying LLM call.
    pub usage: rig::completion::Usage,
    /// Total milliseconds spent waiting for rate-limit permits.
    pub rate_limit_wait_ms: u64,
}

impl<T: std::fmt::Debug> std::fmt::Debug for AnalystInferenceOutcome<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalystInferenceOutcome")
            .field("output", &self.output)
            .field("usage", &self.usage)
            .field("rate_limit_wait_ms", &self.rate_limit_wait_ms)
            .finish()
    }
}

/// Send an analyst inference to the LLM, choosing the correct path based on provider.
///
/// # Provider routing
///
/// - **Native typed providers**: use `prompt_typed_with_retry` (native structured-output).
///   The `parse` hook is NOT called. Runs `validate` on the typed output.
/// - **OpenRouter and DeepSeek**: use `prompt_text_with_retry` (fallback text path, since
///   these providers do not reliably support Scorpio's typed analyst output contract).
///   Runs `parse` on the raw text, then `validate` on the parsed output.
///
/// # Schema failures
///
/// On the text-fallback path, if either `parse` or `validate` returns an error it is
/// returned **immediately** as a `TradingError::SchemaViolation` without retry.
pub(super) async fn run_analyst_inference<T, Parse, Validate>(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    retry_policy: &RetryPolicy,
    max_turns: usize,
    parse: Parse,
    validate: Validate,
) -> Result<AnalystInferenceOutcome<T>, TradingError>
where
    T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
    Parse: Fn(&str) -> Result<T, TradingError>,
    Validate: Fn(&T) -> Result<(), TradingError>,
{
    if matches!(
        agent.provider_id(),
        ProviderId::OpenRouter | ProviderId::DeepSeek
    ) {
        return run_text_fallback_inference(
            agent,
            prompt,
            timeout,
            retry_policy,
            max_turns,
            &parse,
            &validate,
        )
        .await;
    }

    // Native typed-output path (OpenAI, Anthropic, Gemini, Copilot)
    let outcome =
        match prompt_typed_with_retry::<T>(agent, prompt, timeout, retry_policy, max_turns).await {
            Ok(outcome) => outcome,
            Err(err @ TradingError::SchemaViolation { .. })
                if agent.provider_id() == ProviderId::Gemini =>
            {
                return run_text_fallback_inference(
                    agent,
                    prompt,
                    timeout,
                    retry_policy,
                    max_turns,
                    &parse,
                    &validate,
                )
                .await
                .map_err(|fallback_err| match fallback_err {
                    fallback_err @ TradingError::SchemaViolation { .. } => fallback_err,
                    _ => err,
                });
            }
            Err(err) => return Err(err),
        };

    validate(&outcome.result.output)?;
    Ok(AnalystInferenceOutcome {
        output: outcome.result.output,
        usage: outcome.result.usage,
        rate_limit_wait_ms: outcome.rate_limit_wait_ms,
    })
}

async fn run_text_fallback_inference<T, Parse, Validate>(
    agent: &LlmAgent,
    prompt: &str,
    timeout: Duration,
    retry_policy: &RetryPolicy,
    max_turns: usize,
    parse: &Parse,
    validate: &Validate,
) -> Result<AnalystInferenceOutcome<T>, TradingError>
where
    T: schemars::JsonSchema + DeserializeOwned + Send + 'static,
    Parse: Fn(&str) -> Result<T, TradingError>,
    Validate: Fn(&T) -> Result<(), TradingError>,
{
    let outcome = prompt_text_with_retry(agent, prompt, timeout, retry_policy, max_turns).await?;
    let raw = &outcome.result.output;
    if raw.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "{}: LLM returned empty response (model: {})",
                std::any::type_name::<T>()
                    .rsplit("::")
                    .next()
                    .unwrap_or("unknown"),
                agent.model_id(),
            ),
        });
    }

    let output = parse(raw)?;
    validate(&output)?;
    Ok(AnalystInferenceOutcome {
        output,
        usage: outcome.result.usage,
        rate_limit_wait_ms: outcome.rate_limit_wait_ms,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::providers::ProviderId;
    use crate::providers::factory::agent_test_support;
    use std::time::{Duration, Instant};

    use rig::completion::Usage;

    use super::*;

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 45,
            valuation_fetch_timeout_secs: 30,
            retry_max_retries: 4,
            retry_base_delay_ms: 750,
        }
    }

    // ── run_analyst_inference ─────────────────────────────────────────────

    fn fast_policy() -> crate::error::RetryPolicy {
        crate::error::RetryPolicy {
            max_retries: 0,
            base_delay: Duration::from_millis(1),
        }
    }

    fn sample_usage(total: u64) -> Usage {
        Usage {
            input_tokens: total / 2,
            output_tokens: total / 2,
            total_tokens: total,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
        }
    }

    #[tokio::test]
    async fn run_analyst_inference_uses_typed_path_for_non_openrouter() {
        use rig::agent::TypedPromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenAI,
            "m",
            vec![],
            vec![],
        );
        agent.push_typed_ok(TypedPromptResponse::new(
            Output { value: 42 },
            sample_usage(10),
        ));

        let outcome = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |_s: &str| -> Result<Output, crate::error::TradingError> {
                unreachable!("parse hook should not be called on non-OpenRouter path")
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .unwrap();

        assert_eq!(outcome.output.value, 42);
        assert_eq!(agent_test_support::typed_attempts(&agent), 1);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 0);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }

    #[tokio::test]
    async fn run_analyst_inference_uses_text_fallback_for_openrouter() {
        use rig::agent::PromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new(r#"{"value": 99}"#, sample_usage(8)));

        let outcome = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .unwrap();

        assert_eq!(outcome.output.value, 99);
        assert_eq!(agent_test_support::typed_attempts(&agent), 0);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }

    #[tokio::test]
    async fn run_analyst_inference_uses_text_fallback_for_deepseek() {
        use rig::agent::PromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::DeepSeek,
            "deepseek-chat",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new(r#"{"value": 42}"#, sample_usage(8)));

        let outcome = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .unwrap();

        assert_eq!(outcome.output.value, 42);
        assert_eq!(agent_test_support::typed_attempts(&agent), 0);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
        assert_eq!(agent_test_support::prompt_attempts(&agent), 0);
    }

    #[tokio::test]
    async fn run_analyst_inference_returns_schema_violation_for_invalid_fallback_json() {
        use rig::agent::PromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new("not valid json", sample_usage(5)));

        let err = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            crate::error::TradingError::SchemaViolation { .. }
        ));
        // Confirm no retry — still exactly 1 text-turn attempt
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
    }

    #[tokio::test]
    async fn run_analyst_inference_preserves_usage_from_fallback_response() {
        use rig::agent::PromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
        struct Output {
            v: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        agent.push_text_turn_ok(PromptResponse::new(
            r#"{"v": 7}"#,
            Usage {
                input_tokens: 11,
                output_tokens: 13,
                total_tokens: 24,
                cached_input_tokens: 0,
                cache_creation_input_tokens: 0,
            },
        ));

        let outcome = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .unwrap();

        assert_eq!(outcome.usage.input_tokens, 11);
        assert_eq!(outcome.usage.output_tokens, 13);
        assert_eq!(outcome.usage.total_tokens, 24);
    }

    #[tokio::test]
    async fn run_analyst_inference_returns_terminal_schema_violation_for_semantically_invalid_fallback_output()
     {
        use rig::agent::PromptResponse;
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::OpenRouter,
            "openrouter-model",
            vec![],
            vec![],
        );
        // JSON parses fine but fails semantic validation
        agent.push_text_turn_ok(PromptResponse::new(r#"{"value": -1}"#, sample_usage(6)));

        let err = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |o: &Output| -> Result<(), crate::error::TradingError> {
                if o.value < 0 {
                    Err(crate::error::TradingError::SchemaViolation {
                        message: "value must be non-negative".to_owned(),
                    })
                } else {
                    Ok(())
                }
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            crate::error::TradingError::SchemaViolation { .. }
        ));
        // Terminal — no retry
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
    }

    #[tokio::test]
    async fn run_analyst_inference_falls_back_to_text_for_gemini_after_typed_schema_violation() {
        use rig::agent::{PromptResponse, TypedPromptResponse};
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq)]
        struct Output {
            value: i32,
        }

        let (agent, _ctrl) = agent_test_support::mock_llm_agent_with_provider(
            ProviderId::Gemini,
            "gemini-test-model",
            vec![],
            vec![],
        );
        agent.push_typed_error(crate::error::TradingError::SchemaViolation {
            message:
                "provider=gemini model=gemini-test-model: structured output could not be parsed"
                    .to_owned(),
        });
        agent.push_text_turn_ok(PromptResponse::new(r#"{"value": 7}"#, sample_usage(9)));
        // If the implementation retries typed again instead of falling back, this would be used.
        agent.push_typed_ok(TypedPromptResponse::new(
            Output { value: 99 },
            sample_usage(10),
        ));

        let outcome = run_analyst_inference(
            &agent,
            "prompt",
            Duration::from_millis(50),
            &fast_policy(),
            1,
            |s: &str| -> Result<Output, crate::error::TradingError> {
                serde_json::from_str(s).map_err(|e| crate::error::TradingError::SchemaViolation {
                    message: e.to_string(),
                })
            },
            |_o: &Output| -> Result<(), crate::error::TradingError> { Ok(()) },
        )
        .await
        .expect("Gemini should recover via text fallback after a typed schema violation");

        assert_eq!(outcome.output, Output { value: 7 });
        assert_eq!(agent_test_support::typed_attempts(&agent), 1);
        assert_eq!(agent_test_support::text_turn_attempts(&agent), 1);
    }

    #[test]
    fn analyst_runtime_config_uses_symbol_target_and_retry_settings() {
        let llm_config = sample_llm_config();

        let runtime = analyst_runtime_config("AAPL", "2026-03-14", &llm_config);

        assert_eq!(runtime.symbol, "AAPL");
        assert_eq!(runtime.target_date, "2026-03-14");
        assert_eq!(runtime.timeout, Duration::from_secs(45));
        assert_eq!(runtime.retry_policy.max_retries, 4);
        assert_eq!(runtime.retry_policy.base_delay, Duration::from_millis(750));
    }

    // ── validate_summary_content ─────────────────────────────────────────

    // TC-5: baseline — valid input passes
    #[test]
    fn validate_summary_content_passes_for_valid_input() {
        assert!(validate_summary_content("ctx", "A well-formed summary.").is_ok());
    }

    // TC-5: newline and tab are allowed control characters
    #[test]
    fn validate_summary_content_newline_and_tab_are_allowed() {
        let summary = "Line one.\nLine two.\tTabbed.";
        assert!(
            validate_summary_content("ctx", summary).is_ok(),
            "\\n and \\t should be allowed"
        );
    }

    // TC-7: summary containing a NUL control character returns SchemaViolation
    #[test]
    fn validate_summary_content_nul_control_char_returns_schema_violation() {
        let result = validate_summary_content("ctx", "bad\x00content");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // TC-7: ESC control character also rejected
    #[test]
    fn validate_summary_content_escape_control_char_returns_schema_violation() {
        let result = validate_summary_content("ctx", "bad\x1bcontent");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradingError::SchemaViolation { .. }
        ));
    }

    // ── usage_from_response ──────────────────────────────────────────────

    // TC-8: token_counts_available = true when total_tokens > 0
    #[test]
    fn usage_from_response_marks_available_when_total_nonzero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 150,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let result =
            agent_token_usage_from_completion("Agent", "model-x", usage, Instant::now(), 0);
        assert!(
            result.token_counts_available,
            "should be available when total_tokens > 0"
        );
        assert_eq!(result.total_tokens, 150);
    }

    // TC-8: token_counts_available = true when input_tokens > 0 (total may be 0)
    #[test]
    fn usage_from_response_marks_available_when_input_nonzero() {
        let usage = Usage {
            input_tokens: 80,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let result =
            agent_token_usage_from_completion("Agent", "model-x", usage, Instant::now(), 0);
        assert!(
            result.token_counts_available,
            "should be available when input_tokens > 0"
        );
        assert_eq!(result.prompt_tokens, 80);
    }

    // TC-8: token_counts_available = false when all counts are zero
    #[test]
    fn usage_from_response_marks_unavailable_when_all_zero() {
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let result =
            agent_token_usage_from_completion("Agent", "model-x", usage, Instant::now(), 0);
        assert!(
            !result.token_counts_available,
            "should be unavailable when all token counts are zero"
        );
    }

    // Fields are copied correctly
    #[test]
    fn usage_from_response_copies_fields() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
        };
        let result =
            agent_token_usage_from_completion("MyAgent", "my-model", usage, Instant::now(), 0);
        assert_eq!(result.agent_name, "MyAgent");
        assert_eq!(result.model_id, "my-model");
        assert_eq!(result.prompt_tokens, 100);
        assert_eq!(result.completion_tokens, 50);
        assert_eq!(result.total_tokens, 150);
    }
}
