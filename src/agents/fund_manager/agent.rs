use std::time::{Duration, Instant};

use rig::agent::PromptResponse;

use crate::{
    config::{Config, LlmConfig},
    error::{RetryPolicy, TradingError},
    providers::{
        ModelTier,
        factory::{
            CompletionModelHandle, build_agent, create_completion_model, prompt_with_retry_details,
        },
    },
    state::{AgentTokenUsage, Decision, ExecutionStatus, TradingState},
};

use super::{
    prompt::build_prompt_context,
    usage::usage_from_response,
    validation::{
        DETERMINISTIC_REJECT_RATIONALE, deterministic_reject, parse_and_validate_execution_status,
        runtime_timestamp, state_has_missing_inputs,
    },
};

pub(super) trait FundManagerInference {
    async fn infer(
        &self,
        handle: &CompletionModelHandle,
        system_prompt: &str,
        user_prompt: &str,
        timeout: Duration,
        retry_policy: &RetryPolicy,
    ) -> Result<PromptResponse, TradingError>;
}

struct RigFundManagerInference;

impl FundManagerInference for RigFundManagerInference {
    async fn infer(
        &self,
        handle: &CompletionModelHandle,
        system_prompt: &str,
        user_prompt: &str,
        timeout: Duration,
        retry_policy: &RetryPolicy,
    ) -> Result<PromptResponse, TradingError> {
        let agent = build_agent(handle, system_prompt);
        prompt_with_retry_details(&agent, user_prompt, timeout, retry_policy).await
    }
}

/// The Fund Manager Agent.
///
/// Constructs a one-shot prompt from the current [`TradingState`] context, optionally
/// applies the deterministic safety-net, and invokes the `DeepThinking` LLM to produce
/// a validated [`ExecutionStatus`].
pub struct FundManagerAgent {
    handle: CompletionModelHandle,
    symbol: String,
    target_date: String,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl FundManagerAgent {
    /// Construct a new `FundManagerAgent`.
    ///
    /// # Errors
    /// Returns [`TradingError::Config`] if the handle is not for the configured
    /// `deep_thinking_model`.
    pub fn new(
        handle: CompletionModelHandle,
        symbol: impl AsRef<str>,
        target_date: impl AsRef<str>,
        llm_config: &LlmConfig,
    ) -> Result<Self, TradingError> {
        if handle.model_id() != llm_config.deep_thinking_model {
            return Err(TradingError::Config(anyhow::anyhow!(
                "fund manager agent requires deep-thinking model '{}', got '{}'",
                llm_config.deep_thinking_model,
                handle.model_id()
            )));
        }
        Ok(Self {
            handle,
            symbol: symbol.as_ref().to_owned(),
            target_date: target_date.as_ref().to_owned(),
            timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
            retry_policy: RetryPolicy::from_config(llm_config),
        })
    }

    /// Run the Fund Manager: deterministic check → LLM call → validate → write to `state`.
    ///
    /// # Returns
    /// [`AgentTokenUsage`] for the invocation (zero tokens for the deterministic path).
    ///
    /// # Errors
    /// - [`TradingError::SchemaViolation`] when `trader_proposal` is `None` or the LLM
    ///   returns a response that fails domain validation.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    pub async fn run(&self, state: &mut TradingState) -> Result<AgentTokenUsage, TradingError> {
        self.run_with_inference(state, &RigFundManagerInference)
            .await
    }

    pub(super) async fn run_with_inference<I: FundManagerInference>(
        &self,
        state: &mut TradingState,
        inference: &I,
    ) -> Result<AgentTokenUsage, TradingError> {
        let started_at = Instant::now();

        if state.trader_proposal.is_none() {
            return Err(TradingError::SchemaViolation {
                message: "FundManager: trader_proposal is None — cannot render execution decision"
                    .to_owned(),
            });
        }

        if deterministic_reject(state) {
            let decided_at = runtime_timestamp(&state.target_date);
            let status = ExecutionStatus {
                decision: Decision::Rejected,
                rationale: DETERMINISTIC_REJECT_RATIONALE.to_owned(),
                decided_at,
            };
            state.final_execution_status = Some(status);
            return Ok(AgentTokenUsage::unavailable(
                "Fund Manager",
                self.handle.model_id(),
                started_at.elapsed().as_millis() as u64,
            ));
        }

        let (system_prompt, user_prompt) =
            build_prompt_context(state, &self.symbol, &self.target_date);

        let response = inference
            .infer(
                &self.handle,
                &system_prompt,
                &user_prompt,
                self.timeout,
                &self.retry_policy,
            )
            .await?;

        let mut status = parse_and_validate_execution_status(
            &response.output,
            state_has_missing_inputs(state),
            &state.target_date,
        )?;

        status.decided_at = runtime_timestamp(&state.target_date);

        let usage = usage_from_response(
            "Fund Manager",
            self.handle.model_id(),
            response.usage,
            started_at,
        );

        state.final_execution_status = Some(status);
        Ok(usage)
    }
}

pub(super) async fn run_fund_manager(
    state: &mut TradingState,
    config: &Config,
) -> Result<AgentTokenUsage, TradingError> {
    run_fund_manager_with_inference(state, config, &RigFundManagerInference).await
}

pub(super) async fn run_fund_manager_with_inference<I: FundManagerInference>(
    state: &mut TradingState,
    config: &Config,
    inference: &I,
) -> Result<AgentTokenUsage, TradingError> {
    let handle = create_completion_model(ModelTier::DeepThinking, &config.llm, &config.api)?;
    let agent =
        FundManagerAgent::new(handle, &state.asset_symbol, &state.target_date, &config.llm)?;
    agent.run_with_inference(state, inference).await
}
