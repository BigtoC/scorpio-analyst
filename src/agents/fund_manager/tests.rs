use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use rig::{agent::PromptResponse, completion::Usage};
use secrecy::SecretString;

use super::{
    FundManagerAgent,
    agent::{FundManagerInference, run_fund_manager_with_inference},
};
use crate::{
    config::{ApiConfig, Config, LlmConfig, TradingConfig},
    error::{RetryPolicy, TradingError},
    providers::{
        ModelTier,
        factory::{CompletionModelHandle, RetryOutcome},
    },
    state::{
        Decision, FundamentalData, ImpactDirection, MacroEvent, NewsArticle, NewsData, RiskLevel,
        RiskReport, SentimentData, SentimentSource, TechnicalData, TradeAction, TradeProposal,
        TradingState,
    },
};

// ── helpers ──────────────────────────────────────────────────────────────

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

fn sample_api_config() -> ApiConfig {
    ApiConfig {
        openai_api_key: Some(SecretString::from("test-key")),
        ..ApiConfig::default()
    }
}

fn sample_config() -> Config {
    Config {
        llm: sample_llm_config(),
        trading: TradingConfig {
            asset_symbol: "AAPL".to_owned(),
            backtest_start: None,
            backtest_end: None,
        },
        api: sample_api_config(),
        storage: Default::default(),
        rate_limits: Default::default(),
    }
}

fn valid_proposal() -> TradeProposal {
    TradeProposal {
        action: TradeAction::Buy,
        target_price: 185.50,
        stop_loss: 178.00,
        confidence: 0.82,
        rationale: "Strong fundamentals and momentum support this Buy.".to_owned(),
    }
}

fn no_violation_risk_report(level: RiskLevel) -> RiskReport {
    RiskReport {
        risk_level: level,
        assessment: "Risk is within acceptable bounds.".to_owned(),
        recommended_adjustments: vec![],
        flags_violation: false,
    }
}

fn violation_risk_report(level: RiskLevel) -> RiskReport {
    RiskReport {
        risk_level: level,
        assessment: "Material violation detected.".to_owned(),
        recommended_adjustments: vec!["Reject the proposal.".to_owned()],
        flags_violation: true,
    }
}

fn populated_state() -> TradingState {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.trader_proposal = Some(valid_proposal());
    state.aggressive_risk_report = Some(no_violation_risk_report(RiskLevel::Aggressive));
    state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));
    state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));
    state.fundamental_metrics = Some(FundamentalData {
        revenue_growth_pct: Some(0.12),
        pe_ratio: Some(28.5),
        eps: Some(6.1),
        current_ratio: Some(1.3),
        debt_to_equity: Some(0.8),
        gross_margin: Some(0.43),
        net_income: Some(9.5e10),
        insider_transactions: Vec::new(),
        summary: "Strong margins.".to_owned(),
    });
    state.technical_indicators = Some(TechnicalData {
        rsi: Some(58.0),
        macd: None,
        atr: Some(3.1),
        sma_20: Some(182.0),
        sma_50: Some(176.0),
        ema_12: Some(183.0),
        ema_26: Some(178.0),
        bollinger_upper: Some(188.0),
        bollinger_lower: Some(172.0),
        support_level: Some(176.5),
        resistance_level: Some(187.5),
        volume_avg: Some(65_000_000.0),
        summary: "Momentum constructive.".to_owned(),
    });
    state.market_sentiment = Some(SentimentData {
        overall_score: 0.34,
        source_breakdown: vec![SentimentSource {
            source_name: "news".to_owned(),
            score: 0.34,
            sample_size: 12,
        }],
        engagement_peaks: Vec::new(),
        summary: "Modestly positive.".to_owned(),
    });
    state.macro_news = Some(NewsData {
        articles: vec![NewsArticle {
            title: "Apple outlook improves".to_owned(),
            source: "Reuters".to_owned(),
            published_at: "2026-03-14T12:00:00Z".to_owned(),
            relevance_score: Some(0.9),
            snippet: "Demand resilience offsets macro concerns.".to_owned(),
        }],
        macro_events: vec![MacroEvent {
            event: "Fed holds rates".to_owned(),
            impact_direction: ImpactDirection::Neutral,
            confidence: 0.7,
        }],
        summary: "Macro backdrop stable.".to_owned(),
    });
    state
}

fn approved_json() -> String {
    r#"{"decision":"Approved","rationale":"All risk checks passed. Proposal is well-supported by analyst data.","decided_at":"2026-03-15"}"#.to_owned()
}

fn approved_json_without_decided_at() -> String {
    r#"{"decision":"Approved","rationale":"All risk checks passed. Proposal is well-supported by analyst data."}"#.to_owned()
}

fn approved_json_with_missing_data_ack() -> String {
    r#"{"decision":"Approved","rationale":"Approved with reduced confidence because one or more upstream inputs are missing.","decided_at":"2026-03-15"}"#.to_owned()
}

fn rejected_json() -> String {
    r#"{"decision":"Rejected","rationale":"Insufficient supporting evidence for the proposed position size.","decided_at":"2026-03-15"}"#.to_owned()
}

fn make_prompt_response(json: &str, usage: Usage) -> PromptResponse {
    PromptResponse::new(json, usage)
}

fn nonzero_usage() -> Usage {
    Usage {
        input_tokens: 120,
        output_tokens: 45,
        total_tokens: 165,
        cached_input_tokens: 0,
    }
}

fn zero_usage() -> Usage {
    Usage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cached_input_tokens: 0,
    }
}

// ── stub inference ────────────────────────────────────────────────────────

struct StubInference {
    responses: Mutex<VecDeque<Result<RetryOutcome<PromptResponse>, TradingError>>>,
    call_count: Mutex<u32>,
}

impl StubInference {
    fn new(responses: Vec<Result<PromptResponse, TradingError>>) -> Self {
        let wrapped = responses
            .into_iter()
            .map(|r| {
                r.map(|inner| RetryOutcome {
                    result: inner,
                    rate_limit_wait_ms: 0,
                })
            })
            .collect();
        Self {
            responses: Mutex::new(wrapped),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> u32 {
        *self.call_count.lock().unwrap()
    }
}

impl FundManagerInference for StubInference {
    async fn infer(
        &self,
        _handle: &CompletionModelHandle,
        _system_prompt: &str,
        _user_prompt: &str,
        _timeout: Duration,
        _retry_policy: &RetryPolicy,
    ) -> Result<RetryOutcome<PromptResponse>, TradingError> {
        *self.call_count.lock().unwrap() += 1;
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Ok(RetryOutcome {
                    result: make_prompt_response(&approved_json(), zero_usage()),
                    rate_limit_wait_ms: 0,
                })
            })
    }
}

fn fund_manager_for_test() -> FundManagerAgent {
    use crate::providers::factory::create_completion_model;
    let handle = create_completion_model(
        ModelTier::DeepThinking,
        &sample_llm_config(),
        &sample_api_config(),
        &crate::rate_limit::ProviderRateLimiters::default(),
    )
    .unwrap();
    FundManagerAgent::new(handle, "AAPL", "2026-03-15", &sample_llm_config()).unwrap()
}

// ── 4.2: deterministic rejection when both Conservative + Neutral flag ────

#[tokio::test]
async fn deterministic_rejection_when_both_conservative_and_neutral_flag_violation() {
    let mut state = populated_state();
    state.conservative_risk_report = Some(violation_risk_report(RiskLevel::Conservative));
    state.neutral_risk_report = Some(violation_risk_report(RiskLevel::Neutral));

    let inference = StubInference::new(vec![]);
    let agent = fund_manager_for_test();
    let usage = agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    // LLM must NOT have been called.
    assert_eq!(
        inference.call_count(),
        0,
        "LLM must not be invoked for deterministic reject"
    );
    // Decision must be Rejected.
    let status = state.final_execution_status.unwrap();
    assert_eq!(status.decision, Decision::Rejected);
    assert!(
        status.rationale.contains("deterministic") || status.rationale.contains("safety-net"),
        "rationale should mention deterministic rejection: {}",
        status.rationale
    );
    // Usage has no tokens.
    assert!(!usage.token_counts_available);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
    assert_eq!(usage.agent_name, "Fund Manager");
}

// ── 4.3: LLM path when only Conservative flags violation ─────────────────

#[tokio::test]
async fn llm_path_when_only_conservative_flags_violation() {
    let mut state = populated_state();
    state.conservative_risk_report = Some(violation_risk_report(RiskLevel::Conservative));
    state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    assert_eq!(
        inference.call_count(),
        1,
        "LLM must be invoked when only Conservative flags"
    );
    assert!(state.final_execution_status.is_some());
}

#[tokio::test]
async fn llm_path_when_only_neutral_flags_violation() {
    let mut state = populated_state();
    state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));
    state.neutral_risk_report = Some(violation_risk_report(RiskLevel::Neutral));

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    assert_eq!(
        inference.call_count(),
        1,
        "LLM must be invoked when only Neutral flags"
    );
    assert!(state.final_execution_status.is_some());
}

// ── 4.4: LLM path when neither flags violation ───────────────────────────

#[tokio::test]
async fn llm_path_when_neither_flags_violation() {
    let mut state = populated_state();

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    assert_eq!(
        inference.call_count(),
        1,
        "LLM must be invoked when no flags"
    );
    assert!(state.final_execution_status.is_some());
}

// ── 4.5: error when trader_proposal is None ──────────────────────────────

#[tokio::test]
async fn error_when_trader_proposal_is_none() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    // trader_proposal is None by default.

    let inference = StubInference::new(vec![]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        matches!(result, Err(TradingError::SchemaViolation { .. })),
        "expected SchemaViolation, got {result:?}"
    );
    assert!(state.final_execution_status.is_none());
    assert_eq!(
        inference.call_count(),
        0,
        "LLM must not be called when proposal is missing"
    );
}

// ── 4.6: valid Approved ExecutionStatus written to state ─────────────────

#[tokio::test]
async fn approved_execution_status_written_to_state() {
    let mut state = populated_state();

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    let status = state.final_execution_status.as_ref().unwrap();
    assert_eq!(status.decision, Decision::Approved);
    assert!(!status.rationale.is_empty());
}

// ── 4.7: valid Rejected ExecutionStatus written to state ─────────────────

#[tokio::test]
async fn rejected_execution_status_written_to_state() {
    let mut state = populated_state();

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &rejected_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    let status = state.final_execution_status.as_ref().unwrap();
    assert_eq!(status.decision, Decision::Rejected);
    assert!(!status.rationale.is_empty());
}

// ── 4.9: SchemaViolation on invalid decision value ───────────────────────
// The Decision enum is enforced by serde during JSON parsing.

#[tokio::test]
async fn schema_violation_on_invalid_decision_value_from_llm() {
    let mut state = populated_state();
    let bad_json = r#"{"decision":"Maybe","rationale":"Seems fine.","decided_at":"2026-03-15"}"#;
    let inference = StubInference::new(vec![Ok(make_prompt_response(bad_json, nonzero_usage()))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        matches!(result, Err(TradingError::SchemaViolation { .. })),
        "expected SchemaViolation for invalid decision, got {result:?}"
    );
    assert!(state.final_execution_status.is_none());
}

#[tokio::test]
async fn schema_violation_on_unparseable_json_from_llm() {
    let mut state = populated_state();
    let inference = StubInference::new(vec![Ok(make_prompt_response(
        "not-json-at-all",
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        matches!(result, Err(TradingError::SchemaViolation { .. })),
        "expected SchemaViolation for unparseable JSON, got {result:?}"
    );
    assert!(state.final_execution_status.is_none());
}

// ── 4.12: decided_at normalized to runtime timestamp ─────────────────────

#[tokio::test]
async fn decided_at_is_overwritten_with_runtime_timestamp() {
    let mut state = populated_state();
    // LLM returns a far-past decided_at.
    let stale_json =
        r#"{"decision":"Approved","rationale":"Looks good.","decided_at":"1900-01-01T00:00:00Z"}"#;
    let inference = StubInference::new(vec![Ok(make_prompt_response(stale_json, nonzero_usage()))]);
    let agent = fund_manager_for_test();
    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    let decided_at = &state.final_execution_status.as_ref().unwrap().decided_at;
    assert_ne!(
        decided_at, "1900-01-01T00:00:00Z",
        "LLM-provided decided_at must be overwritten by runtime timestamp"
    );
    // Must look like an ISO 8601 string (contains 'T' and ends with 'Z' or '+').
    assert!(
        decided_at.contains('T'),
        "decided_at should be ISO 8601, got: {decided_at}"
    );
}

#[tokio::test]
async fn missing_decided_at_is_filled_by_runtime_timestamp() {
    let mut state = populated_state();
    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json_without_decided_at(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();

    agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    let decided_at = &state.final_execution_status.as_ref().unwrap().decided_at;
    assert!(
        decided_at.contains('T'),
        "missing decided_at should be filled by runtime timestamp, got: {decided_at}"
    );
}

// ── 4.13: AgentTokenUsage populated correctly for LLM path ───────────────

#[tokio::test]
async fn agent_token_usage_populated_for_llm_path() {
    let mut state = populated_state();
    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let usage = agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    assert_eq!(usage.agent_name, "Fund Manager");
    assert_eq!(usage.model_id, "o3");
    assert!(usage.token_counts_available);
    assert_eq!(usage.prompt_tokens, 120);
    assert_eq!(usage.completion_tokens, 45);
    assert_eq!(usage.total_tokens, 165);
    assert!(usage.latency_ms < 5_000);
}

#[tokio::test]
async fn agent_token_usage_marks_unavailable_for_llm_path_without_authoritative_counts() {
    let mut state = populated_state();
    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        zero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let usage = agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();

    assert_eq!(usage.agent_name, "Fund Manager");
    assert_eq!(usage.model_id, "o3");
    assert!(!usage.token_counts_available);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
}

// ── 4.14: AgentTokenUsage for deterministic bypass ───────────────────────

#[tokio::test]
async fn agent_token_usage_for_deterministic_bypass_has_zero_tokens_and_measured_latency() {
    let mut state = populated_state();
    state.conservative_risk_report = Some(violation_risk_report(RiskLevel::Conservative));
    state.neutral_risk_report = Some(violation_risk_report(RiskLevel::Neutral));

    let inference = StubInference::new(vec![]);
    let agent = fund_manager_for_test();
    let start = Instant::now();
    let usage = agent
        .run_with_inference(&mut state, &inference)
        .await
        .unwrap();
    let elapsed = start.elapsed().as_millis() as u64;

    assert!(!usage.token_counts_available);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 0);
    assert_eq!(usage.total_tokens, 0);
    assert!(
        usage.latency_ms <= elapsed + 5,
        "latency_ms {} should be <= elapsed {} + 5ms buffer",
        usage.latency_ms,
        elapsed
    );
    assert_eq!(usage.agent_name, "Fund Manager");
}

// ── 4.15: missing risk reports invoke LLM ────────────────────────────────

#[tokio::test]
async fn missing_risk_reports_invoke_llm_path() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.trader_proposal = Some(valid_proposal());
    // All risk reports are None.

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json_with_missing_data_ack(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        result.is_ok(),
        "should succeed with missing risk reports: {result:?}"
    );
    assert_eq!(
        inference.call_count(),
        1,
        "LLM must be called when risk reports are missing"
    );
    let rationale = &state.final_execution_status.as_ref().unwrap().rationale;
    assert!(
        rationale.contains("missing") || rationale.contains("upstream"),
        "rationale should acknowledge missing risk data: {rationale}"
    );
}

// ── 4.16: missing analyst inputs invoke LLM ──────────────────────────────

#[tokio::test]
async fn missing_analyst_inputs_invoke_llm_path() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.trader_proposal = Some(valid_proposal());
    state.aggressive_risk_report = Some(no_violation_risk_report(RiskLevel::Aggressive));
    state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));
    state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));
    // All analyst fields are None.

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json_with_missing_data_ack(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        result.is_ok(),
        "should succeed with missing analyst inputs: {result:?}"
    );
    assert_eq!(
        inference.call_count(),
        1,
        "LLM must be called when analyst inputs are missing"
    );
    let rationale = &state.final_execution_status.as_ref().unwrap().rationale;
    assert!(
        rationale.contains("missing") || rationale.contains("upstream"),
        "rationale should acknowledge missing analyst data: {rationale}"
    );
}

#[tokio::test]
async fn missing_risk_reports_without_acknowledgment_is_rejected() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.trader_proposal = Some(valid_proposal());

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        matches!(result, Err(TradingError::SchemaViolation { .. })),
        "missing risk reports should require acknowledgment, got {result:?}"
    );
    assert!(state.final_execution_status.is_none());
}

#[tokio::test]
async fn missing_analyst_inputs_without_acknowledgment_is_rejected() {
    let mut state = TradingState::new("AAPL", "2026-03-15");
    state.trader_proposal = Some(valid_proposal());
    state.aggressive_risk_report = Some(no_violation_risk_report(RiskLevel::Aggressive));
    state.neutral_risk_report = Some(no_violation_risk_report(RiskLevel::Neutral));
    state.conservative_risk_report = Some(no_violation_risk_report(RiskLevel::Conservative));

    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);
    let agent = fund_manager_for_test();
    let result = agent.run_with_inference(&mut state, &inference).await;

    assert!(
        matches!(result, Err(TradingError::SchemaViolation { .. })),
        "missing analyst inputs should require acknowledgment, got {result:?}"
    );
    assert!(state.final_execution_status.is_none());
}

// ── constructor: rejects wrong model tier ─────────────────────────────────

#[test]
fn constructor_rejects_wrong_model_id() {
    use crate::providers::factory::create_completion_model;
    let cfg = sample_llm_config();
    let handle = create_completion_model(
        ModelTier::QuickThinking,
        &cfg,
        &sample_api_config(),
        &crate::rate_limit::ProviderRateLimiters::default(),
    )
    .unwrap();
    let result = FundManagerAgent::new(handle, "AAPL", "2026-03-15", &cfg);
    assert!(matches!(result, Err(TradingError::Config(_))));
}

// ── run_fund_manager_with_inference wires up agent and state ─────────────

#[tokio::test]
async fn run_fund_manager_public_entrypoint_works_with_injected_inference() {
    let mut state = populated_state();
    let inference = StubInference::new(vec![Ok(make_prompt_response(
        &approved_json(),
        nonzero_usage(),
    ))]);

    let usage = run_fund_manager_with_inference(&mut state, &sample_config(), &inference)
        .await
        .unwrap();

    assert!(state.final_execution_status.is_some());
    assert_eq!(usage.model_id, "o3");
}
