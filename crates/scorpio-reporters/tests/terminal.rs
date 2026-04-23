use scorpio_core::state::TradingState;
use scorpio_reporters::terminal::render_final_report;

#[test]
fn render_final_report_keeps_core_sections_for_minimal_state() {
    let state = TradingState::new("AAPL", "2026-04-23");
    let report = render_final_report(&state);

    assert!(report.contains("AAPL"));
    assert!(report.contains("Scenario Valuation"));
    assert!(report.contains("Data Quality and Coverage"));
    assert!(report.contains("Evidence Provenance"));
}
