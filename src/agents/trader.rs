//! Trader Agent — Phase 3 of the TradingAgents pipeline.
//!
//! Synthesizes analyst outputs and the researcher debate consensus into a single
//! structured [`TradeProposal`] using a one-shot typed LLM prompt.  The agent
//! uses the `DeepThinking` model tier and writes the validated proposal to
//! [`TradingState::trader_proposal`].

use std::time::Instant;

use crate::{
    config::{Config, LlmConfig},
    error::{RetryPolicy, TradingError},
    providers::{
        ModelTier,
        factory::{
            CompletionModelHandle, build_agent, create_completion_model, prompt_typed_with_retry,
        },
    },
    state::{AgentTokenUsage, TradeProposal, TradingState},
};

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum characters allowed in the `rationale` field.
///
/// Chosen to be compact enough for downstream agents while allowing a full
/// thesis + invalidation-risk explanation.
pub const MAX_RATIONALE_CHARS: usize = 4_096;

/// Maximum tool-turns allowed for the typed prompt (Trader has no tools, so 1 is sufficient).
const MAX_TOOL_TURNS: usize = 1;

/// System prompt for the Trader Agent, adapted from `docs/prompts.md` §3.
const TRADER_SYSTEM_PROMPT: &str = "\
You are the Trader Agent for {ticker} as of {current_date}.
Your job is to synthesize the research consensus and analyst data into a single `TradeProposal` JSON object.

Available inputs:
- Research consensus: {consensus_summary}
- Fundamental data: {fundamental_report}
- Technical data: {technical_report}
- Sentiment data: {sentiment_report}
- News data: {news_report}
- Past learnings: {past_memory_str}

Return ONLY a JSON object matching this exact schema shape:
- `action`: one of `Buy`, `Sell`, `Hold`
- `target_price`: finite number
- `stop_loss`: finite number
- `confidence`: finite number, typically between 0.0 and 1.0
- `rationale`: concise string explaining the trade thesis and main risks

Instructions:
1. Align with the moderator's stance unless the analyst evidence clearly justifies a different conclusion.
2. Make the proposal specific and auditable. Avoid vague wording.
3. Use `rationale` to capture the thesis, the key supporting signals, and the main invalidation risks in compact form.
4. Do not invent fields like entry windows, take-profit ladders, or position size because they are not part of the \
current `TradeProposal` schema.
5. If `action` is `Hold`, you must still provide numeric `target_price` and `stop_loss` because the current schema \
requires them. In that case, use them as monitoring levels: `target_price` for confirmation/re-entry and `stop_loss` \
for thesis-break risk.
6. Return ONLY the single JSON object required by `TradeProposal`.
7. If your proposal diverges from the moderator's consensus stance, you must explicitly explain why in `rationale`.

This proposal will be forwarded to the Risk Management Team. Do not make the final execution decision yourself.";

// ─── Trader Agent ─────────────────────────────────────────────────────────────

/// The Trader Agent.
///
/// Constructs a one-shot typed prompt from the full [`TradingState`] context and
/// invokes the `DeepThinking` LLM to produce a validated [`TradeProposal`].
pub struct TraderAgent {
    handle: CompletionModelHandle,
    timeout: std::time::Duration,
    retry_policy: RetryPolicy,
}

impl TraderAgent {
    /// Construct a new `TraderAgent`.
    ///
    /// # Parameters
    /// - `handle` – pre-constructed `DeepThinking` completion model handle.
    /// - `llm_config` – LLM configuration for timeout and retry policy.
    ///
    /// # Errors
    /// Returns [`TradingError::Config`] if the handle is not for the configured
    /// `deep_thinking_model`.
    pub fn new(
        handle: CompletionModelHandle,
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
            timeout: std::time::Duration::from_secs(llm_config.analyst_timeout_secs),
            retry_policy: RetryPolicy::from_config(llm_config),
            handle,
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
        let started_at = Instant::now();

        let system_prompt = build_system_prompt(state);
        let agent = build_agent(&self.handle, &system_prompt);

        let user_prompt = format!(
            "Produce a TradeProposal JSON for {} as of {}.",
            state.asset_symbol, state.target_date
        );

        let response = prompt_typed_with_retry::<TradeProposal>(
            &agent,
            &user_prompt,
            self.timeout,
            &self.retry_policy,
            MAX_TOOL_TURNS,
        )
        .await?;

        // Post-parse domain validation (fail-fast, non-retriable).
        validate_trade_proposal(&response.output)?;

        let usage = usage_from_typed_response(
            "Trader Agent",
            self.handle.model_id(),
            response.usage,
            started_at,
        );

        state.trader_proposal = Some(response.output);
        Ok(usage)
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Construct a [`TraderAgent`] and run it against `state`.
///
/// This is the primary entry point for the downstream `add-graph-orchestration`
/// change.  It creates a `DeepThinking` completion model handle from `config`,
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
    let handle = create_completion_model(ModelTier::DeepThinking, &config.llm, &config.api)?;
    let agent = TraderAgent::new(handle, &config.llm)?;
    agent.run(state).await
}

// ─── Context helpers ──────────────────────────────────────────────────────────

/// Build the system prompt by substituting all placeholders from `state`.
///
/// Missing analyst outputs are serialized as `"null"`.  A missing
/// `consensus_summary` is replaced with an explicit absence note so the
/// model is aware the debate phase did not produce a consensus.
fn build_system_prompt(state: &TradingState) -> String {
    let consensus = state
        .consensus_summary
        .as_deref()
        .unwrap_or("(no debate consensus available — base the proposal on analyst data alone)");

    let fundamental_report =
        serde_json::to_string(&state.fundamental_metrics).unwrap_or_else(|_| "null".to_owned());
    let technical_report =
        serde_json::to_string(&state.technical_indicators).unwrap_or_else(|_| "null".to_owned());
    let sentiment_report =
        serde_json::to_string(&state.market_sentiment).unwrap_or_else(|_| "null".to_owned());
    let news_report =
        serde_json::to_string(&state.macro_news).unwrap_or_else(|_| "null".to_owned());

    TRADER_SYSTEM_PROMPT
        .replace("{ticker}", &state.asset_symbol)
        .replace("{current_date}", &state.target_date)
        .replace("{consensus_summary}", consensus)
        .replace("{fundamental_report}", &fundamental_report)
        .replace("{technical_report}", &technical_report)
        .replace("{sentiment_report}", &sentiment_report)
        .replace("{news_report}", &news_report)
        .replace("{past_memory_str}", "")
}

// ─── Validation ───────────────────────────────────────────────────────────────

/// Domain-validate a [`TradeProposal`] after successful JSON deserialization.
///
/// All failures return [`TradingError::SchemaViolation`] and are treated as
/// non-retriable (the provider already decoded valid JSON — retrying the same
/// prompt is unlikely to fix a domain constraint violation).
pub(crate) fn validate_trade_proposal(proposal: &TradeProposal) -> Result<(), TradingError> {
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

// ─── Token usage helper ───────────────────────────────────────────────────────

fn usage_from_typed_response(
    agent_name: &str,
    model_id: &str,
    usage: rig::completion::Usage,
    started_at: Instant,
) -> AgentTokenUsage {
    AgentTokenUsage {
        agent_name: agent_name.to_owned(),
        model_id: model_id.to_owned(),
        token_counts_available: usage.total_tokens > 0
            || usage.input_tokens > 0
            || usage.output_tokens > 0,
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        latency_ms: started_at.elapsed().as_millis() as u64,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::{
        config::LlmConfig,
        state::{TradeAction, TradeProposal, TradingState},
    };

    fn sample_llm_config() -> LlmConfig {
        LlmConfig {
            quick_thinking_provider: "openai".to_owned(),
            deep_thinking_provider: "openai".to_owned(),
            quick_thinking_model: "gpt-4o-mini".to_owned(),
            deep_thinking_model: "o3".to_owned(),
            max_debate_rounds: 3,
            max_risk_rounds: 2,
            analyst_timeout_secs: 30,
            retry_max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }

    fn valid_proposal() -> TradeProposal {
        TradeProposal {
            action: TradeAction::Buy,
            target_price: 185.50,
            stop_loss: 178.00,
            confidence: 0.82,
            rationale: "Strong revenue growth and expanding margins support a Buy. \
                Main risk is macro headwinds compressing multiples."
                .to_owned(),
        }
    }

    fn empty_state() -> TradingState {
        TradingState::new("AAPL", "2026-03-15")
    }

    // ── Task 3.1 / 3.2: Valid proposal accepted and written to state ──────

    #[test]
    fn valid_buy_proposal_passes_validation() {
        let proposal = valid_proposal();
        assert!(validate_trade_proposal(&proposal).is_ok());
    }

    // ── Task 3.2: action variants ─────────────────────────────────────────

    #[test]
    fn valid_sell_proposal_passes_validation() {
        let proposal = TradeProposal {
            action: TradeAction::Sell,
            target_price: 160.00,
            stop_loss: 172.00,
            confidence: 0.70,
            rationale: "Deteriorating fundamentals and bearish technicals warrant a Sell."
                .to_owned(),
        };
        assert!(validate_trade_proposal(&proposal).is_ok());
    }

    // ── Task 3.2a: Hold still requires numeric target_price and stop_loss ─

    #[test]
    fn hold_proposal_with_monitoring_levels_passes_validation() {
        let proposal = TradeProposal {
            action: TradeAction::Hold,
            target_price: 190.00, // confirmation / re-entry level
            stop_loss: 175.00,    // thesis-break level
            confidence: 0.55,
            rationale: "Mixed signals. Hold pending clearer macro direction. \
                Re-enter above 190, thesis breaks below 175."
                .to_owned(),
        };
        assert!(validate_trade_proposal(&proposal).is_ok());
        assert_eq!(proposal.action, TradeAction::Hold);
        assert!(proposal.target_price > 0.0);
        assert!(proposal.stop_loss > 0.0);
    }

    // ── Task 3.3: target_price <= 0.0 rejected ────────────────────────────

    #[test]
    fn negative_target_price_rejected() {
        let mut proposal = valid_proposal();
        proposal.target_price = -10.0;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn zero_target_price_rejected() {
        let mut proposal = valid_proposal();
        proposal.target_price = 0.0;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn nan_target_price_rejected() {
        let mut proposal = valid_proposal();
        proposal.target_price = f64::NAN;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    // ── Task 3.4: stop_loss <= 0.0 rejected ──────────────────────────────

    #[test]
    fn zero_stop_loss_rejected() {
        let mut proposal = valid_proposal();
        proposal.stop_loss = 0.0;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn infinite_stop_loss_rejected() {
        let mut proposal = valid_proposal();
        proposal.stop_loss = f64::INFINITY;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    // ── Task 3.5: non-finite confidence rejected ──────────────────────────

    #[test]
    fn nan_confidence_rejected() {
        let mut proposal = valid_proposal();
        proposal.confidence = f64::NAN;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn infinite_confidence_rejected() {
        let mut proposal = valid_proposal();
        proposal.confidence = f64::INFINITY;
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    // ── Task 3.6: empty rationale rejected ───────────────────────────────

    #[test]
    fn empty_rationale_rejected() {
        let mut proposal = valid_proposal();
        proposal.rationale = String::new();
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn whitespace_only_rationale_rejected() {
        let mut proposal = valid_proposal();
        proposal.rationale = "   ".to_owned();
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    // ── Task 3.7: oversized / control-char rationale rejected ────────────

    #[test]
    fn oversized_rationale_rejected() {
        let mut proposal = valid_proposal();
        proposal.rationale = "x".repeat(MAX_RATIONALE_CHARS + 1);
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn rationale_at_exact_limit_accepted() {
        let mut proposal = valid_proposal();
        proposal.rationale = "x".repeat(MAX_RATIONALE_CHARS);
        assert!(validate_trade_proposal(&proposal).is_ok());
    }

    #[test]
    fn control_char_rationale_rejected() {
        let mut proposal = valid_proposal();
        proposal.rationale = "bad\x00content".to_owned();
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn escape_char_rationale_rejected() {
        let mut proposal = valid_proposal();
        proposal.rationale = "bad\x1bcontent".to_owned();
        let result = validate_trade_proposal(&proposal);
        assert!(matches!(result, Err(TradingError::SchemaViolation { .. })));
    }

    #[test]
    fn newline_and_tab_in_rationale_allowed() {
        let mut proposal = valid_proposal();
        proposal.rationale = "Thesis.\nRisk:\tMacro headwinds.".to_owned();
        assert!(validate_trade_proposal(&proposal).is_ok());
    }

    // ── Task 3.8: malformed JSON rejected (unit-level parse test) ────────
    // The typed provider path returns SchemaViolation on failed deserialization.
    // We verify this by attempting to deserialize bad JSON directly.

    #[test]
    fn malformed_json_fails_deserialization() {
        let result = serde_json::from_str::<TradeProposal>("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn json_missing_action_field_fails_deserialization() {
        let json =
            r#"{"target_price": 185.5, "stop_loss": 178.0, "confidence": 0.82, "rationale": "ok"}"#;
        let result = serde_json::from_str::<TradeProposal>(json);
        assert!(result.is_err());
    }

    // ── Task 3.9: AgentTokenUsage recorded with correct agent name ────────

    #[test]
    fn usage_from_typed_response_agent_name_and_model_id() {
        let usage = rig::completion::Usage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cached_input_tokens: 0,
        };
        let result = usage_from_typed_response("Trader Agent", "o3", usage, Instant::now());
        assert_eq!(result.agent_name, "Trader Agent");
        assert_eq!(result.model_id, "o3");
        assert!(result.token_counts_available);
        assert_eq!(result.prompt_tokens, 100);
        assert_eq!(result.completion_tokens, 50);
        assert_eq!(result.total_tokens, 150);
    }

    // ── Task 3.10: token_counts_available = false when all counts are zero ─

    #[test]
    fn usage_from_typed_response_unavailable_when_all_zero() {
        let usage = rig::completion::Usage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cached_input_tokens: 0,
        };
        let result = usage_from_typed_response("Trader Agent", "o3", usage, Instant::now());
        assert!(!result.token_counts_available);
    }

    // ── Task 3.11: prompt contains moderator-alignment and divergence instructions ──

    #[test]
    fn system_prompt_contains_alignment_and_divergence_instructions() {
        assert!(TRADER_SYSTEM_PROMPT.contains("Align with the moderator's stance"));
        assert!(TRADER_SYSTEM_PROMPT.contains("diverges from the moderator's consensus stance"));
        assert!(TRADER_SYSTEM_PROMPT.contains("explicitly explain why in `rationale`"));
    }

    // ── Task 4.1: all four analyst outputs serialized into prompt ────────

    #[test]
    fn system_prompt_includes_all_analyst_placeholders_with_null_when_missing() {
        let state = empty_state();
        let prompt = build_system_prompt(&state);
        assert!(prompt.contains("Fundamental data: null"));
        assert!(prompt.contains("Technical data: null"));
        assert!(prompt.contains("Sentiment data: null"));
        assert!(prompt.contains("News data: null"));
    }

    // ── Task 4.2: missing analyst outputs serialized as "null" ───────────
    // (covered by task 4.1 above — all fields are None in empty_state())

    // ── Task 4.3: missing consensus_summary uses explicit absence note ───

    #[test]
    fn missing_consensus_summary_uses_absence_note() {
        let state = empty_state();
        assert!(state.consensus_summary.is_none());
        let prompt = build_system_prompt(&state);
        assert!(prompt.contains("no debate consensus available"));
        // Must not substitute an empty string
        assert!(!prompt.contains("Research consensus: \n"));
    }

    #[test]
    fn present_consensus_summary_is_injected() {
        let mut state = empty_state();
        state.consensus_summary = Some("Hold — bullish signals balanced by macro risk.".to_owned());
        let prompt = build_system_prompt(&state);
        assert!(prompt.contains("Hold — bullish signals balanced by macro risk."));
    }

    // ── Task 4.4: ticker and current_date substituted from TradingState ──

    #[test]
    fn ticker_and_date_substituted_in_prompt() {
        let state = empty_state();
        let prompt = build_system_prompt(&state);
        assert!(prompt.contains("AAPL"));
        assert!(prompt.contains("2026-03-15"));
    }

    // ── Constructor rejects quick-thinking handle ─────────────────────────

    #[test]
    fn constructor_rejects_wrong_model_id() {
        use crate::{
            config::ApiConfig,
            providers::{ModelTier, factory::create_completion_model},
        };
        use secrecy::SecretString;

        let cfg = sample_llm_config();
        let api_cfg = ApiConfig {
            finnhub_rate_limit: 30,
            openai_api_key: Some(SecretString::from("test-key")),
            anthropic_api_key: None,
            gemini_api_key: None,
            finnhub_api_key: None,
        };
        let handle = create_completion_model(ModelTier::QuickThinking, &cfg, &api_cfg).unwrap();
        let result = TraderAgent::new(handle, &cfg);
        assert!(matches!(result, Err(TradingError::Config(_))));
    }
}
