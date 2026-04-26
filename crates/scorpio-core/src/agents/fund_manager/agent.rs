use std::time::{Duration, Instant};

use graph_flow::Context;
use rig::agent::PromptResponse;

use super::{
    prompt::build_prompt_context,
    validation::{
        parse_and_validate_execution_status, runtime_timestamp, state_has_missing_inputs,
    },
};
use crate::agents::shared::agent_token_usage_from_completion;
use crate::{
    config::{Config, LlmConfig},
    error::{RetryPolicy, TradingError},
    providers::{
        ModelTier,
        factory::{
            CompletionModelHandle, RetryOutcome, build_agent, create_completion_model,
            prompt_with_retry_details,
        },
    },
    rate_limit::ProviderRateLimiters,
    state::{AgentTokenUsage, TradingState},
};

pub(super) trait FundManagerInference {
    async fn infer(
        &self,
        handle: &CompletionModelHandle,
        system_prompt: &str,
        user_prompt: &str,
        timeout: Duration,
        retry_policy: &RetryPolicy,
    ) -> Result<RetryOutcome<PromptResponse>, TradingError>;
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
    ) -> Result<RetryOutcome<PromptResponse>, TradingError> {
        let agent = build_agent(handle, system_prompt);
        prompt_with_retry_details(&agent, user_prompt, timeout, retry_policy).await
    }
}

/// The Fund Manager Agent.
///
/// Constructs a one-shot prompt from the current [`TradingState`] context and invokes
/// the `DeepThinking` LLM to produce a validated `ExecutionStatus`.
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

    /// Run the Fund Manager: LLM call → validate → write to `state`.
    ///
    /// # Returns
    /// [`AgentTokenUsage`] for the invocation.
    ///
    /// # Errors
    /// - [`TradingError::SchemaViolation`] when `trader_proposal` is `None` or the LLM
    ///   returns a response that fails domain validation.
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    pub async fn run(&self, state: &mut TradingState) -> Result<AgentTokenUsage, TradingError> {
        self.run_with_inference(state, true, &RigFundManagerInference)
            .await
    }

    pub(super) async fn run_with_inference<I: FundManagerInference>(
        &self,
        state: &mut TradingState,
        risk_stage_enabled: bool,
        inference: &I,
    ) -> Result<AgentTokenUsage, TradingError> {
        let started_at = Instant::now();

        if state.trader_proposal.is_none() {
            return Err(TradingError::SchemaViolation {
                message: "FundManager: trader_proposal is None — cannot render execution decision"
                    .to_owned(),
            });
        }

        let trader_proposal_action = state
            .trader_proposal
            .as_ref()
            .map(|p| p.action.clone())
            .expect("checked above");
        // Topology-aware dual-risk derivation: if the run was configured with
        // zero risk rounds, the risk stage was deliberately bypassed and the
        // status is `StageDisabled` rather than `Unknown`. The topology is
        // read off the runtime policy that `PreflightTask` hydrates; absent
        // policy or absent required_inputs falls back to the legacy
        // "risk stage enabled" semantic so existing tests stay green.
        let dual_risk_status = crate::agents::risk::DualRiskStatus::from_reports_with_topology(
            state.conservative_risk_report.as_ref(),
            state.neutral_risk_report.as_ref(),
            risk_stage_enabled,
        );

        let (system_prompt, user_prompt) =
            build_prompt_context(state, &self.symbol, &self.target_date, dual_risk_status);

        let outcome = inference
            .infer(
                &self.handle,
                &system_prompt,
                &user_prompt,
                self.timeout,
                &self.retry_policy,
            )
            .await?;

        let mut status = parse_and_validate_execution_status(
            &outcome.result.output,
            state_has_missing_inputs(state, dual_risk_status),
            &state.target_date,
            dual_risk_status,
            trader_proposal_action,
        )?;

        status.decided_at = runtime_timestamp(&state.target_date);

        let usage = agent_token_usage_from_completion(
            "Fund Manager",
            self.handle.model_id(),
            outcome.result.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        state.final_execution_status = Some(status);
        Ok(usage)
    }
}

pub(super) async fn run_fund_manager(
    state: &mut TradingState,
    config: &Config,
    context: &Context,
) -> Result<AgentTokenUsage, TradingError> {
    let routing_flags = context
        .get_sync::<crate::workflow::RoutingFlags>(crate::workflow::KEY_ROUTING_FLAGS)
        .ok_or_else(|| {
            TradingError::Config(anyhow::anyhow!(
                "fund manager: missing routing flags — preflight must run before fund manager"
            ))
        })?;
    run_fund_manager_with_inference(
        state,
        config,
        !routing_flags.skip_risk,
        &RigFundManagerInference,
    )
    .await
}

pub(super) async fn run_fund_manager_with_inference<I: FundManagerInference>(
    state: &mut TradingState,
    config: &Config,
    risk_stage_enabled: bool,
    inference: &I,
) -> Result<AgentTokenUsage, TradingError> {
    let handle = create_completion_model(
        ModelTier::DeepThinking,
        &config.llm,
        &config.providers,
        &ProviderRateLimiters::from_config(&config.providers),
    )?;
    let agent =
        FundManagerAgent::new(handle, &state.asset_symbol, &state.target_date, &config.llm)?;
    agent
        .run_with_inference(state, risk_stage_enabled, inference)
        .await
}
