#![cfg(feature = "test-helpers")]

#[path = "support/workflow_observability_pipeline_support.rs"]
mod workflow_observability_pipeline_support;

use workflow_observability_pipeline_support::{
    EventCollector, StructuredEventCollector, run_stubbed_pipeline_under_collector,
    run_stubbed_pipeline_under_structured_collector,
};

#[test]
fn fund_manager_decision_event_excludes_rationale() {
    let collector = StructuredEventCollector::new();
    let final_state =
        run_stubbed_pipeline_under_structured_collector(collector.clone(), "obs-fund.db");

    let fields = collector.collected_fields();
    let rationale_text = "stub: approved - risk within tolerances";
    let has_rationale = fields.iter().any(|(_, val)| val.contains(rationale_text));

    assert!(
        final_state.final_execution_status.is_some(),
        "fund manager path should complete successfully before checking rationale leakage"
    );
    assert!(
        !has_rationale,
        "rationale text must NOT appear in any tracing event field value, but found it in fields: {fields:?}"
    );
}

#[test]
fn tracing_emits_cycle_start_and_complete_events() {
    let collector = EventCollector::new();
    run_stubbed_pipeline_under_collector(collector.clone(), "obs-cycle.db");

    let events = collector.collected();
    let has_start = events.iter().any(|e| e.contains("cycle started"));
    let has_complete = events.iter().any(|e| e.contains("cycle complete"));
    assert!(
        has_start,
        "expected 'cycle started' event, got events: {events:?}"
    );
    assert!(
        has_complete,
        "expected 'cycle complete' event, got events: {events:?}"
    );
}

#[test]
fn tracing_emits_phase_name_field_for_analyst_phase() {
    let collector = StructuredEventCollector::new();
    run_stubbed_pipeline_under_structured_collector(collector.clone(), "obs-phases.db");

    let fields = collector.collected_fields();
    let phase_names: Vec<&str> = fields
        .iter()
        .filter(|(name, _)| name == "phase_name")
        .map(|(_, val)| val.as_str())
        .collect();

    assert!(
        phase_names.contains(&"analyst_team"),
        "expected phase_name 'analyst_team' from real AnalystSyncTask, got phase_names: {phase_names:?}"
    );
}

#[test]
fn tracing_emits_snapshot_saved_events_from_pipeline() {
    let collector = EventCollector::new();
    run_stubbed_pipeline_under_collector(collector.clone(), "obs-snapshot.db");

    let events = collector.collected();
    let snapshot_count = events
        .iter()
        .filter(|e| e.contains("snapshot saved"))
        .count();
    assert!(
        snapshot_count >= 1,
        "expected at least 1 'snapshot saved' event, got {snapshot_count}. Events: {events:?}"
    );
}
