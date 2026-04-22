//! Data Quality and Coverage section for the final terminal report.

use std::fmt::Write;

use scorpio_core::state::{DataCoverageReport, TradingState};

/// Render the `Data Quality and Coverage` section into `out`.
///
/// - When `state.data_coverage` is `None`: emits the exact string `Unavailable`.
/// - When `state.data_coverage` is `Some(coverage)`:
///   - Lists `required_inputs` explicitly.
///   - Emits a `Missing inputs:` bulleted list when non-empty, otherwise `Missing inputs: none`.
///   - When all issue lists are empty, emits an "all present" confirmation line.
///
/// Never panics — all `Option` accesses use pattern matching.
pub(crate) fn write_data_quality_and_coverage(out: &mut String, state: &TradingState) {
    super::final_report::section_header(out, "Data Quality and Coverage");

    match state.data_coverage.as_ref() {
        None => {
            let _ = writeln!(out, "Unavailable");
        }
        Some(coverage) => {
            write_coverage_body(out, coverage);
        }
    }
}

fn write_coverage_body(out: &mut String, coverage: &DataCoverageReport) {
    // Required inputs line
    let required_list = coverage.required_inputs.join(", ");
    let _ = writeln!(out, "Required inputs: {required_list}");

    // Missing inputs
    if coverage.missing_inputs.is_empty() {
        let _ = writeln!(out, "Missing inputs: none");
    } else {
        let _ = writeln!(out, "Missing inputs:");
        for item in &coverage.missing_inputs {
            let _ = writeln!(out, "  - {item}");
        }
    }

    // All-present confirmation
    if coverage.missing_inputs.is_empty() {
        let _ = writeln!(out, "All required inputs are present.");
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_core::state::TradingState;

    fn state_with_coverage(coverage: DataCoverageReport) -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-04-03");
        state.data_coverage = Some(coverage);
        state
    }

    fn full_coverage() -> DataCoverageReport {
        DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec![],
        }
    }

    #[test]
    fn write_data_quality_and_coverage_shows_unavailable_when_none() {
        let state = TradingState::new("AAPL", "2026-04-03");
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(
            out.contains("Data Quality and Coverage"),
            "section heading must appear"
        );
        assert!(
            out.contains("Unavailable"),
            "must render Unavailable when None"
        );
    }

    #[test]
    fn write_data_quality_and_coverage_lists_required_inputs() {
        let state = state_with_coverage(full_coverage());
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(
            out.contains("Required inputs: fundamentals, sentiment, news, technical"),
            "must list all required inputs from the struct"
        );
    }

    #[test]
    fn write_data_quality_and_coverage_lists_missing_inputs() {
        let coverage = DataCoverageReport {
            required_inputs: vec![
                "fundamentals".to_owned(),
                "sentiment".to_owned(),
                "news".to_owned(),
                "technical".to_owned(),
            ],
            missing_inputs: vec!["technical".to_owned()],
        };
        let state = state_with_coverage(coverage);
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(
            out.contains("Missing inputs:"),
            "must have missing inputs label"
        );
        assert!(
            out.contains("  - technical"),
            "must bullet-list missing inputs"
        );
    }

    #[test]
    fn write_data_quality_and_coverage_shows_all_present_when_no_missing() {
        let state = state_with_coverage(full_coverage());
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(
            out.contains("All required inputs are present"),
            "must confirm all inputs present when no issues"
        );
    }

    #[test]
    fn write_data_quality_and_coverage_heading_always_appears() {
        // None case
        let state = TradingState::new("TSLA", "2026-04-03");
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(out.contains("Data Quality and Coverage"));

        // Some case
        let state2 = state_with_coverage(full_coverage());
        let mut out2 = String::new();
        write_data_quality_and_coverage(&mut out2, &state2);
        assert!(out2.contains("Data Quality and Coverage"));
    }
}
