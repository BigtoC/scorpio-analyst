#![cfg(feature = "test-helpers")]

mod workflow_observability_task_support;

use graph_flow::Context;
use workflow_observability_task_support::{
    EventCollector, StructuredEventCollector, run_analyst_sync_under_collector,
    run_analyst_sync_under_structured_collector, run_debate_accounting_under_collector,
    run_debate_accounting_under_structured_collector, run_risk_accounting_under_collector,
};

#[test]
fn tracing_emits_phase_completion_event_for_analyst_sync() {
    let collector = EventCollector::new();
    run_analyst_sync_under_collector(collector.clone());

    let events = collector.collected();
    let has_phase_complete = events
        .iter()
        .any(|e| e.contains("AnalystSyncTask") && e.contains("phase 1 complete"));

    assert!(
        has_phase_complete,
        "expected a tracing event containing 'AnalystSyncTask: phase 1 complete', but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_structured_failures_field_for_analyst_sync() {
    let collector = StructuredEventCollector::new();
    run_analyst_sync_under_structured_collector(collector.clone());

    let fields = collector.collected_fields();
    let failures_field = fields.iter().find(|(name, _)| name == "failures");

    assert!(
        failures_field.is_some(),
        "expected a structured field named 'failures' to be emitted, but got fields: {fields:?}"
    );

    let (_, failures_value) = failures_field.expect("failures field should exist");
    assert_eq!(
        failures_value, "0",
        "with all analysts succeeding, 'failures' field must be '0', got '{failures_value}'"
    );
}

#[tokio::test]
async fn debate_round_transitions_are_context_observable() {
    use scorpio_analyst::workflow::test_support::{KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS};

    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 2u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    for expected_round in 1u32..=2 {
        let current: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        ctx.set(KEY_DEBATE_ROUND, current + 1).await;

        let new_round: u32 = ctx.get(KEY_DEBATE_ROUND).await.unwrap_or(0);
        assert_eq!(new_round, expected_round);

        let max: u32 = ctx.get(KEY_MAX_DEBATE_ROUNDS).await.unwrap_or(0);
        let should_loop = new_round < max;

        if expected_round < 2 {
            assert!(should_loop, "round {expected_round}: loop should continue");
        } else {
            assert!(
                !should_loop,
                "round {expected_round}: loop should stop (advance to trader)"
            );
        }
    }
}

#[test]
fn tracing_emits_task_started_for_analyst_sync() {
    let collector = EventCollector::new();
    run_analyst_sync_under_collector(collector.clone());

    let events = collector.collected();
    let has_started = events.iter().any(|e| e.contains("task started"));
    assert!(
        has_started,
        "expected a tracing event containing 'task started', but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_phase_complete_for_analyst_sync() {
    let collector = EventCollector::new();
    run_analyst_sync_under_collector(collector.clone());

    let events = collector.collected();
    let has_phase_complete = events.iter().any(|e| e.contains("phase complete"));
    assert!(
        has_phase_complete,
        "expected a tracing event containing 'phase complete', but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_snapshot_saved_for_analyst_sync() {
    let collector = EventCollector::new();
    run_analyst_sync_under_collector(collector.clone());

    let events = collector.collected();
    let has_snapshot = events.iter().any(|e| e.contains("snapshot saved"));
    assert!(
        has_snapshot,
        "expected a tracing event containing 'snapshot saved', but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_phase_number_field_for_analyst_sync() {
    let collector = StructuredEventCollector::new();
    run_analyst_sync_under_structured_collector(collector.clone());

    let fields = collector.collected_fields();
    let phase_field = fields
        .iter()
        .find(|(name, val)| name == "phase" && val == "1");
    assert!(
        phase_field.is_some(),
        "expected a structured field 'phase' = '1', but got fields: {fields:?}"
    );
}

#[test]
fn tracing_emits_debate_round_complete_event() {
    let collector = EventCollector::new();
    run_debate_accounting_under_collector(collector.clone(), 1, 0);

    let events = collector.collected();
    let has_round = events.iter().any(|e| e.contains("debate round complete"));
    assert!(
        has_round,
        "expected 'debate round complete' event, but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_risk_round_complete_event() {
    let collector = EventCollector::new();
    run_risk_accounting_under_collector(collector.clone(), 1, 0);

    let events = collector.collected();
    let has_round = events.iter().any(|e| e.contains("risk round complete"));
    assert!(
        has_round,
        "expected 'risk round complete' event, but got events: {events:?}"
    );
}

#[test]
fn tracing_emits_structured_round_field_for_debate() {
    let collector = StructuredEventCollector::new();
    run_debate_accounting_under_structured_collector(collector.clone(), 2, 0);

    let fields = collector.collected_fields();
    let has_round = fields
        .iter()
        .any(|(name, val)| name == "round" && val == "1");
    let has_max = fields
        .iter()
        .any(|(name, val)| name == "max_rounds" && val == "2");
    assert!(
        has_round,
        "expected structured field 'round' = '1', got fields: {fields:?}"
    );
    assert!(
        has_max,
        "expected structured field 'max_rounds' = '2', got fields: {fields:?}"
    );
}

#[test]
fn tracing_zero_round_debate_no_round_event() {
    let collector = EventCollector::new();
    run_debate_accounting_under_collector(collector.clone(), 0, 0);

    let events = collector.collected();
    let has_round = events.iter().any(|e| e.contains("debate round complete"));
    assert!(
        !has_round,
        "zero-round debate must NOT emit 'debate round complete', but got events: {events:?}"
    );
}
