use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::agents::shared::{agent_token_usage_from_completion, extract_json_object};
use crate::error::{RetryPolicy, TradingError};
use crate::providers::factory::{
    CompletionModelHandle, build_agent, prompt_with_retry_validated_details,
};
use crate::state::auditor::{AuditorReport, Finding, Severity};
use crate::state::{AgentTokenUsage, DataCoverageReport, TradeAction, TradingState};

pub(crate) mod prompt;
pub(crate) use prompt::build_system_prompt;

// ── Untrusted text label ──────────────────────────────────────────────────────

#[derive(Serialize)]
struct UntrustedText<'a> {
    kind: &'static str,
    content: &'a str,
}

impl<'a> UntrustedText<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            kind: "external_model_text",
            content,
        }
    }
}

// ── Curated audit input view ──────────────────────────────────────────────────

#[derive(Serialize)]
struct AuditDebateMessage<'a> {
    role: &'a str,
    content: UntrustedText<'a>,
}

#[derive(Serialize)]
struct AuditRiskReport<'a> {
    risk_level: String,
    flags_violation: bool,
    assessment: UntrustedText<'a>,
}

#[derive(Serialize)]
struct AuditRiskView<'a> {
    aggressive: Option<AuditRiskReport<'a>>,
    conservative: Option<AuditRiskReport<'a>>,
    neutral: Option<AuditRiskReport<'a>>,
}

#[derive(Serialize)]
struct AuditTradeProposal<'a> {
    action: String,
    target_price: f64,
    stop_loss: f64,
    confidence: f64,
    rationale: UntrustedText<'a>,
}

#[derive(Serialize)]
struct AuditExecutionStatus<'a> {
    decision: String,
    action: String,
    rationale: UntrustedText<'a>,
}

#[derive(Serialize)]
struct AuditorInputView<'a> {
    ticker: &'a str,
    current_price: Option<f64>,
    trader_proposal: Option<AuditTradeProposal<'a>>,
    final_execution_status: Option<AuditExecutionStatus<'a>>,
    data_coverage: Option<&'a DataCoverageReport>,
    debate_history: Vec<AuditDebateMessage<'a>>,
    risk_reports: AuditRiskView<'a>,
    deterministic_findings: &'a [Finding],
}

fn audit_input_view<'a>(
    state: &'a TradingState,
    deterministic: &'a [Finding],
) -> AuditorInputView<'a> {
    const MAX_DEBATE_MSGS: usize = 20;

    let debate_history: Vec<_> = state
        .debate_history
        .iter()
        .rev()
        .take(MAX_DEBATE_MSGS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|msg| AuditDebateMessage {
            role: &msg.role,
            content: UntrustedText::new(&msg.content),
        })
        .collect();

    let risk_reports = AuditRiskView {
        aggressive: state
            .aggressive_risk_report
            .as_ref()
            .map(|r| AuditRiskReport {
                risk_level: format!("{:?}", r.risk_level),
                flags_violation: r.flags_violation,
                assessment: UntrustedText::new(&r.assessment),
            }),
        conservative: state
            .conservative_risk_report
            .as_ref()
            .map(|r| AuditRiskReport {
                risk_level: format!("{:?}", r.risk_level),
                flags_violation: r.flags_violation,
                assessment: UntrustedText::new(&r.assessment),
            }),
        neutral: state.neutral_risk_report.as_ref().map(|r| AuditRiskReport {
            risk_level: format!("{:?}", r.risk_level),
            flags_violation: r.flags_violation,
            assessment: UntrustedText::new(&r.assessment),
        }),
    };

    AuditorInputView {
        ticker: &state.asset_symbol,
        current_price: state.current_price,
        trader_proposal: state.trader_proposal.as_ref().map(|p| AuditTradeProposal {
            action: format!("{:?}", p.action),
            target_price: p.target_price,
            stop_loss: p.stop_loss,
            confidence: p.confidence,
            rationale: UntrustedText::new(&p.rationale),
        }),
        final_execution_status: state.final_execution_status.as_ref().map(|s| {
            AuditExecutionStatus {
                decision: format!("{:?}", s.decision),
                action: format!("{:?}", s.action),
                rationale: UntrustedText::new(&s.rationale),
            }
        }),
        data_coverage: state.data_coverage.as_ref(),
        debate_history,
        risk_reports,
        deterministic_findings: deterministic,
    }
}

// ── Deterministic checks ──────────────────────────────────────────────────────

/// Run local deterministic checks against the final `TradingState`.
///
/// Runs before and independent of the LLM call so findings survive in the
/// fail-open path even when semantic review is unavailable.
pub(crate) fn run_deterministic_checks(state: &TradingState) -> Vec<Finding> {
    let mut findings = Vec::new();

    let Some(proposal) = &state.trader_proposal else {
        return findings;
    };

    if let Some(current) = state.current_price {
        if matches!(proposal.action, TradeAction::Buy) && proposal.target_price < current {
            findings.push(Finding {
                severity: Severity::Critical,
                location: "trader_proposal.target_price".into(),
                description: format!(
                    "BUY proposal target_price ({:.2}) is below current_price ({:.2})",
                    proposal.target_price, current
                ),
                excerpt: Some(format!(
                    "target_price={:.2}, current_price={:.2}",
                    proposal.target_price, current
                )),
            });
        }
        if matches!(proposal.action, TradeAction::Sell) && proposal.target_price > current {
            findings.push(Finding {
                severity: Severity::Critical,
                location: "trader_proposal.target_price".into(),
                description: format!(
                    "SELL proposal target_price ({:.2}) is above current_price ({:.2})",
                    proposal.target_price, current
                ),
                excerpt: Some(format!(
                    "target_price={:.2}, current_price={:.2}",
                    proposal.target_price, current
                )),
            });
        }
    }

    if matches!(proposal.action, TradeAction::Buy) && proposal.stop_loss > proposal.target_price {
        findings.push(Finding {
            severity: Severity::Critical,
            location: "trader_proposal.stop_loss".into(),
            description: format!(
                "BUY stop_loss ({:.2}) exceeds target_price ({:.2})",
                proposal.stop_loss, proposal.target_price
            ),
            excerpt: None,
        });
    }

    findings
}

// ── Model output and parsing ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct ModelAuditorOutput {
    pub(crate) findings: Vec<Finding>,
    pub(crate) summary: String,
}

pub(crate) fn parse_auditor_output(raw: &str) -> Result<ModelAuditorOutput, TradingError> {
    let cleaned = extract_json_object("Auditor", raw)?;
    serde_json::from_str::<ModelAuditorOutput>(&cleaned).map_err(|_| {
        TradingError::SchemaViolation {
            message: "Auditor: response could not be parsed as AuditorReport".to_owned(),
        }
    })
}

fn validate_auditor_output(raw: &str) -> Result<(), TradingError> {
    parse_auditor_output(raw).map(|_| ())
}

fn merge_with_runtime_metadata(
    mut model_output: ModelAuditorOutput,
    deterministic: Vec<Finding>,
    model_id: &str,
) -> AuditorReport {
    let det_locations: HashSet<String> = deterministic.iter().map(|f| f.location.clone()).collect();
    // Remove LLM findings that duplicate deterministic check locations.
    model_output
        .findings
        .retain(|f| !det_locations.contains(&f.location));
    let mut all_findings: Vec<Finding> = deterministic;
    all_findings.extend(model_output.findings);
    all_findings.truncate(20);

    AuditorReport {
        findings: all_findings,
        summary: model_output.summary,
        audited_at: Utc::now(),
        auditor_model_id: model_id.to_owned(),
    }
}

// ── Agent ─────────────────────────────────────────────────────────────────────

pub struct AuditorAgent {
    handle: CompletionModelHandle,
    timeout: Duration,
    retry_policy: RetryPolicy,
}

impl AuditorAgent {
    pub fn new(
        handle: CompletionModelHandle,
        timeout: Duration,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            handle,
            timeout,
            retry_policy,
        }
    }

    /// Audit the completed `TradingState`.
    ///
    /// Runs deterministic checks first, then a curated LLM pass. Returns the
    /// merged `AuditorReport` and token accounting. Callers must handle
    /// the error path as `AuditStatus::FailedOpen` — this function itself does
    /// not mutate state.
    ///
    /// # Errors
    ///
    /// Returns [`TradingError`] on LLM failure or JSON parse failure after
    /// the retry budget is exhausted. Deterministic findings are lost on this
    /// path; callers should capture them before calling.
    pub async fn audit(
        &self,
        state: &TradingState,
    ) -> Result<(AuditorReport, AgentTokenUsage), TradingError> {
        let deterministic = run_deterministic_checks(state);
        let view = audit_input_view(state, &deterministic);
        let user_payload =
            serde_json::to_string(&view).map_err(|e| TradingError::Config(e.into()))?;
        let started_at = Instant::now();
        let system_prompt = build_system_prompt(state)?;
        let agent = build_agent(&self.handle, &system_prompt);
        let outcome = prompt_with_retry_validated_details(
            &agent,
            &user_payload,
            self.timeout,
            &self.retry_policy,
            validate_auditor_output,
        )
        .await?;

        let usage = agent_token_usage_from_completion(
            "Auditor",
            self.handle.model_id(),
            outcome.result.usage,
            started_at,
            outcome.rate_limit_wait_ms,
        );
        let model_output = parse_auditor_output(&outcome.result.output)?;
        let report =
            merge_with_runtime_metadata(model_output, deterministic, self.handle.model_id());
        Ok((report, usage))
    }
}

pub(crate) async fn run_auditor(
    state: &TradingState,
    config: &crate::config::Config,
) -> Result<(AuditorReport, AgentTokenUsage), TradingError> {
    use crate::providers::{ModelTier, factory::create_completion_model};
    use crate::rate_limit::ProviderRateLimiters;
    let handle = create_completion_model(
        ModelTier::QuickThinking,
        &config.llm,
        &config.providers,
        &ProviderRateLimiters::from_config(&config.providers),
    )?;
    let agent = AuditorAgent::new(
        handle,
        Duration::from_secs(config.llm.analyst_timeout_secs),
        RetryPolicy::from_config(&config.llm),
    );
    agent.audit(state).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::auditor::{Finding, Severity};
    use crate::state::{TradeAction, TradeProposal, TradingState};

    fn dummy_report(severities: &[Severity]) -> AuditorReport {
        AuditorReport {
            findings: severities
                .iter()
                .map(|s| Finding {
                    severity: *s,
                    location: "test".into(),
                    description: "test".into(),
                    excerpt: None,
                })
                .collect(),
            summary: "x".into(),
            audited_at: chrono::Utc::now(),
            auditor_model_id: "test".into(),
        }
    }

    fn make_state_with_buy_target_below_current_price() -> TradingState {
        let mut state = TradingState::new("TEST".to_owned(), "2026-05-10".to_owned());
        state.current_price = Some(120.0);
        state.trader_proposal = Some(TradeProposal {
            action: TradeAction::Buy,
            target_price: 100.0,
            stop_loss: 95.0,
            confidence: 0.8,
            rationale: "test rationale".to_owned(),
            valuation_assessment: None,
            scenario_valuation: None,
        });
        state
    }

    #[test]
    fn report_has_no_critical_findings_is_false_when_critical_exists() {
        let report = dummy_report(&[Severity::Critical]);
        assert!(!report.has_no_critical_findings());
    }

    #[test]
    fn report_has_no_critical_findings_is_true_when_no_critical_exists() {
        let report = dummy_report(&[Severity::Warning, Severity::Info]);
        assert!(report.has_no_critical_findings());
    }

    #[test]
    fn parse_extracts_json_from_fenced_block() {
        let raw = "Some preamble.\n```json\n{\"findings\": [], \"summary\": \"ok\"}\n```";
        let report = parse_auditor_output(raw).unwrap();
        assert!(report.findings.is_empty());
    }

    #[test]
    fn deterministic_checks_flag_buy_target_below_current_price() {
        let state = make_state_with_buy_target_below_current_price();
        let findings = run_deterministic_checks(&state);
        assert!(findings.iter().any(|f| f.severity == Severity::Critical));
    }

    #[test]
    fn deterministic_checks_empty_when_no_proposal() {
        let state = TradingState::new("AAPL".to_owned(), "2026-05-10".to_owned());
        assert!(run_deterministic_checks(&state).is_empty());
    }

    #[test]
    fn deterministic_checks_no_finding_when_buy_target_above_current() {
        let mut state = TradingState::new("AAPL".to_owned(), "2026-05-10".to_owned());
        state.current_price = Some(100.0);
        state.trader_proposal = Some(TradeProposal {
            action: TradeAction::Buy,
            target_price: 120.0,
            stop_loss: 90.0,
            confidence: 0.7,
            rationale: "valid".to_owned(),
            valuation_assessment: None,
            scenario_valuation: None,
        });
        let findings = run_deterministic_checks(&state);
        assert!(findings.is_empty());
    }

    #[test]
    fn merge_caps_findings_at_twenty() {
        let deterministic: Vec<Finding> = (0..15)
            .map(|i| Finding {
                severity: Severity::Critical,
                location: format!("det_{i}"),
                description: "det".into(),
                excerpt: None,
            })
            .collect();
        let model_output = ModelAuditorOutput {
            findings: (0..10)
                .map(|i| Finding {
                    severity: Severity::Warning,
                    location: format!("llm_{i}"),
                    description: "llm".into(),
                    excerpt: None,
                })
                .collect(),
            summary: "test".into(),
        };
        let report = merge_with_runtime_metadata(model_output, deterministic, "test-model");
        assert_eq!(report.findings.len(), 20);
    }
}
