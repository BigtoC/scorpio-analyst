//! Data Quality and Coverage section for the final terminal report.

use std::fmt::Write;

use scorpio_core::state::{DataCoverageReport, TradingState};

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
    let required_list = coverage.required_inputs.join(", ");
    let _ = writeln!(out, "Required inputs: {required_list}");

    if coverage.missing_inputs.is_empty() {
        let _ = writeln!(out, "Missing inputs: none");
    } else {
        let _ = writeln!(out, "Missing inputs:");
        for item in &coverage.missing_inputs {
            let _ = writeln!(out, "  - {item}");
        }
    }

    if coverage.missing_inputs.is_empty() {
        let _ = writeln!(out, "All required inputs are present.");
    }
}

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
        assert!(out.contains("Data Quality and Coverage"));
        assert!(out.contains("Unavailable"));
    }

    #[test]
    fn write_data_quality_and_coverage_lists_required_inputs() {
        let state = state_with_coverage(full_coverage());
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(out.contains("Required inputs: fundamentals, sentiment, news, technical"));
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
        assert!(out.contains("Missing inputs:"));
        assert!(out.contains("  - technical"));
    }

    #[test]
    fn write_data_quality_and_coverage_shows_all_present_when_no_missing() {
        let state = state_with_coverage(full_coverage());
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(out.contains("All required inputs are present"));
    }

    #[test]
    fn write_data_quality_and_coverage_heading_always_appears() {
        let state = TradingState::new("TSLA", "2026-04-03");
        let mut out = String::new();
        write_data_quality_and_coverage(&mut out, &state);
        assert!(out.contains("Data Quality and Coverage"));

        let state2 = state_with_coverage(full_coverage());
        let mut out2 = String::new();
        write_data_quality_and_coverage(&mut out2, &state2);
        assert!(out2.contains("Data Quality and Coverage"));
    }
}
