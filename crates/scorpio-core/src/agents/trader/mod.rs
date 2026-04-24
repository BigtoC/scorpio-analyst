//! Trader Agent - Phase 3 of the TradingAgents pipeline.
//!
//! Synthesizes analyst outputs and the researcher debate consensus into a single
//! structured [`TradeProposal`] using a one-shot typed LLM prompt. The agent
//! uses the `DeepThinking` model tier and writes the validated proposal to
//! [`TradingState::trader_proposal`].

use std::time::{Duration, Instant};

use rig::agent::TypedPromptResponse;

#[cfg(test)]
use crate::agents::shared::redact_secret_like_values;
use crate::{
    agents::shared::{
        agent_token_usage_from_completion, sanitize_date_for_prompt, sanitize_symbol_for_prompt,
    },
    config::{Config, LlmConfig},
    constants::{MAX_RATIONALE_CHARS, TRADER_MAX_TURNS},
    error::{RetryPolicy, TradingError},
    providers::{
        ModelTier,
        factory::{
            CompletionModelHandle, RetryOutcome, build_agent, create_completion_model,
            prompt_typed_with_retry,
        },
    },
    rate_limit::ProviderRateLimiters,
    state::{AgentTokenUsage, TradeAction, TradeProposal, TradingState},
};

mod prompt;
mod schema;
#[cfg(test)]
mod tests;
use prompt::build_prompt_context;
use schema::TraderProposalResponse;

trait TraderInference {
    async fn infer(
        &self,
        handle: &CompletionModelHandle,
        system_prompt: &str,
        user_prompt: &str,
        timeout: Duration,
        retry_policy: &RetryPolicy,
    ) -> Result<RetryOutcome<TypedPromptResponse<TraderProposalResponse>>, TradingError>;
}

struct RigTraderInference;

impl TraderInference for RigTraderInference {
    async fn infer(
        &self,
        handle: &CompletionModelHandle,
        system_prompt: &str,
        user_prompt: &str,
        timeout: Duration,
        retry_policy: &RetryPolicy,
    ) -> Result<RetryOutcome<TypedPromptResponse<TraderProposalResponse>>, TradingError> {
        let agent = build_agent(handle, system_prompt);
        prompt_typed_with_retry::<TraderProposalResponse>(
            &agent,
            user_prompt,
            timeout,
            retry_policy,
            TRADER_MAX_TURNS,
        )
        .await
    }
}

/// The Trader Agent.
///
/// Constructs a one-shot typed prompt from the current [`TradingState`] context
/// and invokes the `DeepThinking` LLM to produce a validated [`TradeProposal`].
pub struct TraderAgent {
    handle: CompletionModelHandle,
    symbol: String,
    target_date: String,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl TraderAgent {
    /// Construct a new `TraderAgent`.
    ///
    /// # Parameters
    /// - `handle` - pre-constructed `DeepThinking` completion model handle.
    /// - `symbol` - asset ticker symbol for prompt construction.
    /// - `target_date` - analysis date for prompt construction.
    /// - `llm_config` - LLM configuration for timeout and retry policy.
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
                "trader agent requires deep-thinking model '{}', got '{}'",
                llm_config.deep_thinking_model,
                handle.model_id()
            )));
        }

        Ok(Self {
            handle,
            symbol: sanitize_symbol_for_prompt(symbol.as_ref()),
            target_date: sanitize_date_for_prompt(target_date.as_ref()),
            timeout: Duration::from_secs(llm_config.analyst_timeout_secs),
            retry_policy: RetryPolicy::from_config(llm_config),
        })
    }

    /// Run the Trader Agent: prompt the LLM, validate the response, and write to `state`.
    ///
    /// # Returns
    /// [`AgentTokenUsage`] for the single LLM invocation.
    ///
    /// # Errors
    /// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
    /// - [`TradingError::SchemaViolation`] when the LLM returns a response that
    ///   fails provider-layer JSON decoding or trader-layer domain validation.
    pub async fn run(&self, state: &mut TradingState) -> Result<AgentTokenUsage, TradingError> {
        self.run_with_inference(state, &RigTraderInference).await
    }

    async fn run_with_inference<I: TraderInference>(
        &self,
        state: &mut TradingState,
        inference: &I,
    ) -> Result<AgentTokenUsage, TradingError> {
        let started_at = Instant::now();
        let prompt_context = build_prompt_context(state, &self.symbol, &self.target_date);

        let outcome = inference
            .infer(
                &self.handle,
                &prompt_context.system_prompt,
                &prompt_context.user_prompt,
                self.timeout,
                &self.retry_policy,
            )
            .await?;

        let llm_proposal: TradeProposal = outcome.result.output.into();
        validate_trade_proposal(&llm_proposal)?;
        validate_trade_proposal_context(state, &llm_proposal)?;

        let usage = agent_token_usage_from_completion(
            "Trader Agent",
            self.handle.model_id(),
            outcome.result.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );

        // Inject runtime-owned scenario_valuation from derived_valuation state.
        // The LLM must not author this field (validated above); the runtime stamps
        // the deterministic valuation computed before trader inference.
        let mut proposal = llm_proposal;
        proposal.scenario_valuation = state.derived_valuation().map(|dv| dv.scenario.clone());

        state.trader_proposal = Some(proposal);
        Ok(usage)
    }
}

/// Construct a [`TraderAgent`] and run it against `state`.
///
/// This is the primary entry point for the downstream `add-graph-orchestration`
/// change. It creates a `DeepThinking` completion model handle from `config`,
/// constructs the agent, and invokes it.
///
/// # Returns
/// [`AgentTokenUsage`] so the upstream orchestrator can incorporate it into a
/// "Trader Synthesis" [`PhaseTokenUsage`][crate::state::PhaseTokenUsage] entry.
///
/// # Errors
/// - [`TradingError::Config`] for provider or model configuration problems.
/// - [`TradingError::Rig`] / [`TradingError::NetworkTimeout`] for LLM failures.
/// - [`TradingError::SchemaViolation`] for invalid LLM output.
pub async fn run_trader(
    state: &mut TradingState,
    config: &Config,
) -> Result<AgentTokenUsage, TradingError> {
    run_trader_with_inference(state, config, &RigTraderInference).await
}

async fn run_trader_with_inference<I: TraderInference>(
    state: &mut TradingState,
    config: &Config,
    inference: &I,
) -> Result<AgentTokenUsage, TradingError> {
    let handle = create_completion_model(
        ModelTier::DeepThinking,
        &config.llm,
        &config.providers,
        &ProviderRateLimiters::from_config(&config.providers),
    )?;
    let agent = TraderAgent::new(handle, &state.asset_symbol, &state.target_date, &config.llm)?;
    agent.run_with_inference(state, inference).await
}

/// Domain-validate a [`TradeProposal`] after successful JSON deserialization.
///
/// All failures return [`TradingError::SchemaViolation`] and are treated as
/// non-retriable.
pub(crate) fn validate_trade_proposal(proposal: &TradeProposal) -> Result<(), TradingError> {
    if proposal.scenario_valuation.is_some() {
        return Err(TradingError::SchemaViolation {
            message: "TraderAgent: scenario_valuation is runtime-owned and must not be authored by the LLM"
                .to_owned(),
        });
    }
    if !proposal.target_price.is_finite() || proposal.target_price <= 0.0 {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "TraderAgent: target_price must be finite and > 0, got {}",
                proposal.target_price
            ),
        });
    }
    if !proposal.stop_loss.is_finite() || proposal.stop_loss <= 0.0 {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "TraderAgent: stop_loss must be finite and > 0, got {}",
                proposal.stop_loss
            ),
        });
    }
    if !proposal.confidence.is_finite() {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "TraderAgent: confidence must be finite, got {}",
                proposal.confidence
            ),
        });
    }
    if proposal.rationale.trim().is_empty() {
        return Err(TradingError::SchemaViolation {
            message: "TraderAgent: rationale must not be empty".to_owned(),
        });
    }
    if proposal.rationale.chars().count() > MAX_RATIONALE_CHARS {
        return Err(TradingError::SchemaViolation {
            message: format!(
                "TraderAgent: rationale exceeds maximum {} characters",
                MAX_RATIONALE_CHARS
            ),
        });
    }
    if proposal
        .rationale
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(TradingError::SchemaViolation {
            message: "TraderAgent: rationale contains disallowed control characters".to_owned(),
        });
    }
    Ok(())
}

fn validate_trade_proposal_context(
    state: &TradingState,
    proposal: &TradeProposal,
) -> Result<(), TradingError> {
    let missing_inputs = state.fundamental_metrics().is_none()
        || state.technical_indicators().is_none()
        || state.market_sentiment().is_none()
        || state.macro_news().is_none()
        || state.consensus_summary.is_none();

    if missing_inputs && !rationale_acknowledges_missing_data(&proposal.rationale) {
        return Err(TradingError::SchemaViolation {
            message: "TraderAgent: rationale must acknowledge missing upstream data when analyst inputs or consensus are absent"
                .to_owned(),
        });
    }

    if let Some(consensus_action) = extract_consensus_stance(state.consensus_summary.as_deref())
        && consensus_action != proposal.action
        && !rationale_explains_divergence(&proposal.rationale)
    {
        return Err(TradingError::SchemaViolation {
            message:
                "TraderAgent: rationale must explain divergence from moderator consensus stance"
                    .to_owned(),
        });
    }

    Ok(())
}

fn extract_consensus_stance(consensus_summary: Option<&str>) -> Option<TradeAction> {
    let summary = consensus_summary?;
    summary
        .split(|c: char| !c.is_ascii_alphabetic())
        .find_map(|token| match token {
            "Buy" => Some(TradeAction::Buy),
            "Sell" => Some(TradeAction::Sell),
            "Hold" => Some(TradeAction::Hold),
            _ => None,
        })
}

fn rationale_acknowledges_missing_data(rationale: &str) -> bool {
    let rationale = rationale.to_ascii_lowercase();
    ["missing", "unavailable", "absent", "gap", "limited", "lack"]
        .iter()
        .any(|needle| rationale.contains(needle))
}

fn rationale_explains_divergence(rationale: &str) -> bool {
    let rationale = rationale.to_ascii_lowercase();
    [
        "because",
        "despite",
        "however",
        "although",
        "outweigh",
        "outweighed",
        "diverg",
        "contrary",
        "consensus",
    ]
    .iter()
    .any(|needle| rationale.contains(needle))
}
