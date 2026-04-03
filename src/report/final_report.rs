use std::fmt::Write;

use colored::Colorize;
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};

use crate::state::{Decision, RiskReport, TokenUsageTracker, TradeAction, TradingState};

/// Render a comprehensive terminal report from the completed trading state.
pub fn format_final_report(state: &TradingState) -> String {
    let mut out = String::new();

    write_header(&mut out, state);
    write_executive_summary(&mut out, state);
    write_trader_proposal(&mut out, state);
    write_analyst_snapshot(&mut out, state);
    write_research_debate(&mut out, state);
    write_risk_review(&mut out, state);
    write_safety_check(&mut out, state);
    write_token_usage(&mut out, &state.token_usage);
    write_disclaimer(&mut out);

    out
}

// ── helpers ──────────────────────────────────────────────────────────────

fn first_sentence(s: &str) -> &str {
    for (i, c) in s.char_indices() {
        if matches!(c, '.' | '!' | '?') {
            let after = i + c.len_utf8();
            // End-of-string or followed by whitespace => potential sentence boundary
            let at_boundary = after >= s.len() || s[after..].starts_with(char::is_whitespace);
            if !at_boundary {
                continue;
            }
            // Skip abbreviations: single lowercase letter preceded by another period,
            // e.g. "e.g." or "i.e." — the inner periods are followed by a letter, so
            // they were already skipped; only the trailing period reaches here, but it
            // looks like "g." where the char before is a letter preceded by ".".
            // Heuristic: if the char immediately before the period is a lowercase ASCII
            // letter AND two chars back is a period, treat it as an abbreviation.
            if i > 0 {
                let before = &s[..i];
                let mut chars_rev = before.chars().rev();
                // Skip abbreviations: single lowercase letter preceded by another period,
                // e.g. "e.g." — "g." has a lowercase char before the period, and
                // the char before that is itself a period.
                if let Some(prev_char) = chars_rev.next()
                    && prev_char.is_ascii_lowercase()
                    && chars_rev.next() == Some('.')
                {
                    // Looks like an abbreviation (e.g., "g." in "e.g.")
                    continue;
                }
            }
            return &s[..after];
        }
    }
    s
}

fn action_colored(action: &TradeAction) -> String {
    match action {
        TradeAction::Buy => "Buy".green().bold().to_string(),
        TradeAction::Sell => "Sell".red().bold().to_string(),
        TradeAction::Hold => "Hold".yellow().bold().to_string(),
    }
}

fn decision_colored(decision: &Decision) -> String {
    match decision {
        Decision::Approved => "Approved".green().bold().to_string(),
        Decision::Rejected => "Rejected".red().bold().to_string(),
    }
}

fn confidence_colored(confidence: f64) -> String {
    let label = format!("{confidence:.2}");
    if confidence > 0.7 {
        label.green().to_string()
    } else if confidence >= 0.4 {
        label.yellow().to_string()
    } else {
        label.red().to_string()
    }
}

fn data_status_label(present: bool) -> String {
    if present {
        "Complete".green().to_string()
    } else {
        "Missing".dimmed().to_string()
    }
}

fn violation_label(report: Option<&RiskReport>) -> String {
    match report {
        Some(r) if r.flags_violation => "Yes".red().to_string(),
        Some(_) => "No".green().to_string(),
        None => "Unknown".dimmed().to_string(),
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

fn section_header(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n{}", title.bold().underline());
}

// ── section writers ──────────────────────────────────────────────────────

fn write_header(out: &mut String, state: &TradingState) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}",
        format!("Trading Decision: {}", state.asset_symbol)
            .bold()
            .on_bright_black()
    );
    let _ = writeln!(
        out,
        "As of: {}  |  Execution ID: {}",
        state.target_date, state.execution_id
    );

    if let Some(exec) = &state.final_execution_status {
        let _ = writeln!(
            out,
            "Decision: {}  |  Action: {}  |  Timestamp: {}",
            decision_colored(&exec.decision),
            action_colored(&exec.action),
            exec.decided_at,
        );
    }
}

fn write_executive_summary(out: &mut String, state: &TradingState) {
    section_header(out, "Executive Summary");
    match &state.final_execution_status {
        Some(exec) => {
            let _ = writeln!(out, "{}", exec.rationale);
        }
        None => {
            let _ = writeln!(out, "{}", "No execution status available.".dimmed());
        }
    }
}

fn write_trader_proposal(out: &mut String, state: &TradingState) {
    section_header(out, "Trader Proposal");
    match &state.trader_proposal {
        Some(proposal) => {
            let mut table = Table::new();
            table.set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("Field").add_attribute(Attribute::Bold),
                Cell::new("Value").add_attribute(Attribute::Bold),
            ]);
            table.add_row(vec!["Action".to_owned(), action_colored(&proposal.action)]);
            table.add_row(vec![
                "Confidence".to_owned(),
                confidence_colored(proposal.confidence),
            ]);
            table.add_row(vec![
                "Target Price".to_owned(),
                format!("{:.2}", proposal.target_price),
            ]);
            table.add_row(vec![
                "Stop Loss".to_owned(),
                format!("{:.2}", proposal.stop_loss),
            ]);
            let _ = writeln!(out, "{table}");
            let _ = writeln!(out, "\n{} {}", "Rationale:".bold(), proposal.rationale);
        }
        None => {
            let _ = writeln!(out, "{}", "Trader proposal: unavailable.".dimmed());
        }
    }
}

fn write_analyst_snapshot(out: &mut String, state: &TradingState) {
    section_header(out, "Analyst Evidence Snapshot");

    let analysts: Vec<(&str, Option<&str>, bool)> = vec![
        (
            "Fundamentals",
            state
                .fundamental_metrics
                .as_ref()
                .map(|d| d.summary.as_str()),
            state.fundamental_metrics.is_some(),
        ),
        (
            "Sentiment",
            state
                .market_sentiment
                .as_ref()
                .map(|d| d.summary.as_str()),
            state.market_sentiment.is_some(),
        ),
        (
            "News",
            state.macro_news.as_ref().map(|d| d.summary.as_str()),
            state.macro_news.is_some(),
        ),
        (
            "Technical",
            state
                .technical_indicators
                .as_ref()
                .map(|d| d.summary.as_str()),
            state.technical_indicators.is_some(),
        ),
    ];

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Analyst").add_attribute(Attribute::Bold),
        Cell::new("Key Evidence").add_attribute(Attribute::Bold),
        Cell::new("Status").add_attribute(Attribute::Bold),
    ]);

    for (name, summary, present) in &analysts {
        let evidence = summary.map_or("-", |s| first_sentence(s));
        table.add_row(vec![
            Cell::new(name),
            Cell::new(evidence),
            Cell::new(data_status_label(*present)),
        ]);
    }
    let _ = writeln!(out, "{table}");

    // Detail blocks: full summaries below the table
    for (name, summary, present) in &analysts {
        if *present && let Some(full) = summary {
            let _ = writeln!(out, "\n  {} {}", format!("[{name}]").bold(), full);
        }
    }
}

fn write_research_debate(out: &mut String, state: &TradingState) {
    section_header(out, "Research Debate Summary");

    match &state.consensus_summary {
        Some(consensus) => {
            let _ = writeln!(out, "{} {}", "Consensus:".bold(), consensus);
        }
        None => {
            let _ = writeln!(out, "{}", "No consensus produced.".dimmed());
        }
    }

    // Extract last bull and bear messages from debate history
    let last_bull = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bullish_researcher");
    let last_bear = state
        .debate_history
        .iter()
        .rev()
        .find(|m| m.role == "bearish_researcher");

    if let Some(bull) = last_bull {
        let _ = writeln!(
            out,
            "{} {}",
            "Strongest Bullish Evidence:".bold(),
            first_sentence(&bull.content)
        );
    }
    if let Some(bear) = last_bear {
        let _ = writeln!(
            out,
            "{} {}",
            "Strongest Bearish Evidence:".bold(),
            first_sentence(&bear.content)
        );
    }
}

fn write_risk_review(out: &mut String, state: &TradingState) {
    section_header(out, "Risk Review");

    let personas: Vec<(&str, Option<&RiskReport>)> = vec![
        ("Aggressive", state.aggressive_risk_report.as_ref()),
        ("Neutral", state.neutral_risk_report.as_ref()),
        ("Conservative", state.conservative_risk_report.as_ref()),
    ];

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Persona").add_attribute(Attribute::Bold),
        Cell::new("Violation").add_attribute(Attribute::Bold),
        Cell::new("Assessment").add_attribute(Attribute::Bold),
        Cell::new("Adjustments").add_attribute(Attribute::Bold),
    ]);

    for (name, report) in &personas {
        let (assessment, adjustments) = match report {
            Some(r) => {
                let adj = if r.recommended_adjustments.is_empty() {
                    "None".to_owned()
                } else {
                    r.recommended_adjustments.join(", ")
                };
                (first_sentence(&r.assessment).to_owned(), adj)
            }
            None => ("Unknown".to_owned(), "Unknown".to_owned()),
        };
        table.add_row(vec![
            Cell::new(name),
            Cell::new(violation_label(*report)),
            Cell::new(assessment),
            Cell::new(adjustments),
        ]);
    }
    let _ = writeln!(out, "{table}");

    // Detail blocks: full assessments below the table
    for (name, report) in &personas {
        if let Some(r) = report {
            let _ = writeln!(out, "\n  {} {}", format!("[{name}]").bold(), r.assessment);
        }
    }
}

fn write_safety_check(out: &mut String, state: &TradingState) {
    section_header(out, "Deterministic Safety Check");

    let neutral_flag = state
        .neutral_risk_report
        .as_ref()
        .map(|r| r.flags_violation);
    let conservative_flag = state
        .conservative_risk_report
        .as_ref()
        .map(|r| r.flags_violation);
    let auto_reject = neutral_flag == Some(true) && conservative_flag == Some(true);

    let flag_str = |f: Option<bool>| -> String {
        match f {
            Some(true) => "true".red().to_string(),
            Some(false) => "false".green().to_string(),
            None => "unknown".dimmed().to_string(),
        }
    };

    let _ = writeln!(out, "  Neutral flags violation: {}", flag_str(neutral_flag));
    let _ = writeln!(
        out,
        "  Conservative flags violation: {}",
        flag_str(conservative_flag)
    );
    let auto_reject_label = if auto_reject {
        "Yes".red().bold().to_string()
    } else {
        "No".green().to_string()
    };
    let _ = writeln!(out, "  Auto-reject rule triggered: {auto_reject_label}");
}

fn write_token_usage(out: &mut String, tracker: &TokenUsageTracker) {
    section_header(out, "Token Usage Summary");

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        Cell::new("Phase").add_attribute(Attribute::Bold),
        Cell::new("Prompt")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Completion")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Total")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new("Duration")
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
    ]);

    for phase in &tracker.phase_usage {
        table.add_row(vec![
            Cell::new(&phase.phase_name),
            Cell::new(phase.phase_prompt_tokens).set_alignment(CellAlignment::Right),
            Cell::new(phase.phase_completion_tokens).set_alignment(CellAlignment::Right),
            Cell::new(phase.phase_total_tokens).set_alignment(CellAlignment::Right),
            Cell::new(format_duration_ms(phase.phase_duration_ms))
                .set_alignment(CellAlignment::Right),
        ]);
    }

    // Totals row
    table.add_row(vec![
        Cell::new("TOTAL").add_attribute(Attribute::Bold),
        Cell::new(tracker.total_prompt_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(tracker.total_completion_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(tracker.total_tokens)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
        Cell::new(""),
    ]);

    let _ = writeln!(out, "{table}");
}

fn write_disclaimer(out: &mut String) {
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", "Disclaimers".bold().dimmed());
    let disclaimer = "\
- This is AI-generated analysis for educational and research purposes only.
- Not financial advice. Market data may be incomplete or delayed.
- Past performance does not guarantee future results.";
    let _ = writeln!(out, "{}", disclaimer.dimmed());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        Decision, ExecutionStatus, PhaseTokenUsage, TradeAction, TradeProposal, TradingState,
    };

    fn minimal_state() -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-04-03");
        state.final_execution_status = Some(ExecutionStatus {
            decision: Decision::Approved,
            action: TradeAction::Buy,
            rationale: "Approved based on strong fundamentals.".to_owned(),
            decided_at: "2026-04-03T12:00:00Z".to_owned(),
        });
        state.trader_proposal = Some(TradeProposal {
            action: TradeAction::Buy,
            target_price: 190.0,
            stop_loss: 175.0,
            confidence: 0.8,
            rationale: "Strong growth and momentum.".to_owned(),
        });
        state
    }

    #[test]
    fn format_final_report_contains_symbol_and_date() {
        let state = minimal_state();
        let report = format_final_report(&state);
        assert!(report.contains("AAPL"));
        assert!(report.contains("2026-04-03"));
    }

    #[test]
    fn format_final_report_handles_missing_analysts_gracefully() {
        let state = minimal_state();
        let report = format_final_report(&state);
        // Should contain "Missing" for analysts that are None
        assert!(report.contains("Missing") || report.contains("missing"));
    }

    #[test]
    fn format_final_report_shows_token_totals() {
        let mut state = minimal_state();
        state.token_usage.push_phase_usage(PhaseTokenUsage {
            phase_name: "Test Phase".to_owned(),
            agent_usage: vec![],
            phase_prompt_tokens: 100,
            phase_completion_tokens: 50,
            phase_total_tokens: 150,
            phase_duration_ms: 2500,
        });
        let report = format_final_report(&state);
        assert!(report.contains("Test Phase"));
        assert!(report.contains("150"));
    }

    #[test]
    fn format_final_report_contains_disclaimer() {
        let state = minimal_state();
        let report = format_final_report(&state);
        assert!(report.contains("educational"));
    }

    #[test]
    fn first_sentence_extracts_up_to_period() {
        assert_eq!(first_sentence("Hello world. More text here."), "Hello world.");
    }

    #[test]
    fn first_sentence_returns_full_string_without_boundary() {
        assert_eq!(first_sentence("No period here"), "No period here");
    }

    #[test]
    fn first_sentence_handles_abbreviation_like_patterns() {
        // "e.g." has periods not followed by whitespace, so it continues
        assert_eq!(first_sentence("Use e.g. this. Then more."), "Use e.g. this.");
    }

    #[test]
    fn format_duration_ms_formats_seconds() {
        assert_eq!(format_duration_ms(2500), "2.5s");
        assert_eq!(format_duration_ms(500), "500ms");
    }
}
