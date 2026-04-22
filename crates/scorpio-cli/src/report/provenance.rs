//! Evidence Provenance section for the final terminal report.

use std::fmt::Write;

use scorpio_core::state::{ProvenanceSummary, TradingState};

/// Render the `Evidence Provenance` section into `out`.
///
/// - When `state.provenance_summary` is `None`: emits the exact string `Unavailable`.
/// - When `state.provenance_summary` is `Some(provenance)`:
///   - Lists `providers_used` as a labeled inline list, or `Providers: none` if empty.
///   - `generated_at` is intentionally omitted (Stage 1 — the report header carries the timestamp).
///
/// Never panics — all `Option` accesses use pattern matching.
pub(crate) fn write_evidence_provenance(out: &mut String, state: &TradingState) {
    super::final_report::section_header(out, "Evidence Provenance");

    match state.provenance_summary.as_ref() {
        None => {
            let _ = writeln!(out, "Unavailable");
        }
        Some(provenance) => {
            write_provenance_body(out, provenance);
        }
    }
}

fn write_provenance_body(out: &mut String, provenance: &ProvenanceSummary) {
    // Providers line
    if provenance.providers_used.is_empty() {
        let _ = writeln!(out, "Providers: none");
    } else {
        let providers_list = provenance.providers_used.join(", ");
        let _ = writeln!(out, "Providers: {providers_list}");
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use scorpio_core::state::TradingState;

    fn state_with_provenance(provenance: ProvenanceSummary) -> TradingState {
        let mut state = TradingState::new("AAPL", "2026-04-03");
        state.provenance_summary = Some(provenance);
        state
    }

    #[test]
    fn write_evidence_provenance_shows_unavailable_when_none() {
        let state = TradingState::new("AAPL", "2026-04-03");
        let mut out = String::new();
        write_evidence_provenance(&mut out, &state);
        assert!(
            out.contains("Evidence Provenance"),
            "section heading must appear"
        );
        assert!(
            out.contains("Unavailable"),
            "must render Unavailable when provenance_summary is None"
        );
    }

    #[test]
    fn write_evidence_provenance_lists_providers() {
        let provenance = ProvenanceSummary {
            providers_used: vec![
                "finnhub".to_owned(),
                "fred".to_owned(),
                "yfinance".to_owned(),
            ],
        };
        let state = state_with_provenance(provenance);
        let mut out = String::new();
        write_evidence_provenance(&mut out, &state);
        assert!(out.contains("Providers: finnhub, fred, yfinance"));
    }

    #[test]
    fn write_evidence_provenance_shows_none_when_providers_empty() {
        let provenance = ProvenanceSummary {
            providers_used: vec![],
        };
        let state = state_with_provenance(provenance);
        let mut out = String::new();
        write_evidence_provenance(&mut out, &state);
        assert!(out.contains("Providers: none"));
    }

    #[test]
    fn write_evidence_provenance_heading_always_appears() {
        // None case
        let state = TradingState::new("TSLA", "2026-04-03");
        let mut out = String::new();
        write_evidence_provenance(&mut out, &state);
        assert!(out.contains("Evidence Provenance"));

        // Some case with providers
        let provenance = ProvenanceSummary {
            providers_used: vec!["finnhub".to_owned()],
        };
        let state2 = state_with_provenance(provenance);
        let mut out2 = String::new();
        write_evidence_provenance(&mut out2, &state2);
        assert!(out2.contains("Evidence Provenance"));
    }
}
