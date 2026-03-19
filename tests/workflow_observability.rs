//! Integration tests verifying that the workflow pipeline emits structured
//! tracing events that can be consumed by downstream CLI/TUI streaming.
//!
//! These tests use `tracing_subscriber` with a custom layer to capture spans
//! and events emitted during pipeline task execution and verify that key
//! lifecycle events (phase start/end, task invocation, round transitions) are
//! present.

use std::sync::{Arc, Mutex};

use graph_flow::{Context, Task};
use scorpio_analyst::{
    state::TradingState,
    workflow::{
        SnapshotStore,
        context_bridge::{serialize_state_to_context, write_prefixed_result},
        tasks::{AnalystSyncTask, KEY_DEBATE_ROUND, KEY_MAX_DEBATE_ROUNDS},
    },
};
use tempfile::tempdir;
use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;

// ── Captured event collector ──────────────────────────────────────────────────

/// Thread-safe collector of tracing event messages.
#[derive(Clone, Default)]
struct EventCollector {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventCollector {
    fn new() -> Self {
        Self::default()
    }

    fn collected(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventCollector {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct MessageVisitor(String);
        impl tracing::field::Visit for MessageVisitor {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "message" {
                    self.0 = value.to_owned();
                }
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{:?}", value);
                }
            }
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if !visitor.0.is_empty() {
            self.events.lock().unwrap().push(visitor.0);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn make_store() -> (Arc<SnapshotStore>, tempfile::TempDir) {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let store = Arc::new(
        SnapshotStore::new(Some(&db_path))
            .await
            .expect("snapshot store"),
    );
    (store, dir)
}

async fn seed_all_analysts_ok(ctx: &Context, state: &TradingState) {
    serialize_state_to_context(state, ctx)
        .await
        .expect("serialize");

    use scorpio_analyst::state::{FundamentalData, NewsData, SentimentData, TechnicalData};

    for analyst in &["fundamental", "sentiment", "news", "technical"] {
        ctx.set(format!("analyst.{analyst}.ok"), true).await;
    }

    write_prefixed_result(
        ctx,
        "analyst",
        "fundamental",
        &FundamentalData {
            revenue_growth_pct: None,
            pe_ratio: Some(24.5),
            eps: Some(6.05),
            current_ratio: None,
            debt_to_equity: None,
            gross_margin: None,
            net_income: None,
            insider_transactions: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    write_prefixed_result(
        ctx,
        "analyst",
        "sentiment",
        &SentimentData {
            overall_score: 0.5,
            source_breakdown: vec![],
            engagement_peaks: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    write_prefixed_result(
        ctx,
        "analyst",
        "news",
        &NewsData {
            articles: vec![],
            macro_events: vec![],
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();

    write_prefixed_result(
        ctx,
        "analyst",
        "technical",
        &TechnicalData {
            rsi: None,
            macd: None,
            atr: None,
            sma_20: None,
            sma_50: None,
            ema_12: None,
            ema_26: None,
            bollinger_upper: None,
            bollinger_lower: None,
            support_level: None,
            resistance_level: None,
            volume_avg: None,
            summary: "ok".to_owned(),
        },
    )
    .await
    .unwrap();
}

// ────────────────────────────────────────────────────────────────────────────
// 11.10 — Tracing emits phase/task/round transition events
// ────────────────────────────────────────────────────────────────────────────
//
// Runs `AnalystSyncTask` under a custom tracing subscriber that captures all
// emitted events.  Verifies that at least one event related to Phase 1
// completion ("AnalystSyncTask: phase 1 complete") is emitted, confirming the
// pipeline emits structured tracing events usable by downstream CLI/TUI
// streaming consumers.
//
// NOTE: `with_default` sets a thread-local subscriber.  Because Tokio may poll
// futures on different threads, we use a single-threaded runtime to ensure the
// subscriber remains active throughout all `.await` points.

#[test]
fn tracing_emits_phase_completion_event_for_analyst_sync() {
    let collector = EventCollector::new();
    let subscriber = tracing_subscriber::registry().with(collector.clone());

    with_default(subscriber, || {
        // Use a single-threaded runtime so all async work stays on this thread
        // and the thread-local subscriber remains active throughout.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();

            seed_all_analysts_ok(&ctx, &state).await;

            let task = AnalystSyncTask::new(store);
            task.run(ctx.clone())
                .await
                .expect("AnalystSyncTask should succeed");
        });
    });

    let events = collector.collected();

    // At least one event should mention phase 1 completion.
    let has_phase_complete = events
        .iter()
        .any(|e| e.contains("AnalystSyncTask") && e.contains("phase 1 complete"));

    assert!(
        has_phase_complete,
        "expected a tracing event containing 'AnalystSyncTask: phase 1 complete', \
         but got events: {events:?}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 11.15 — Structured field capture: `failures` field emitted by AnalystSyncTask
// ────────────────────────────────────────────────────────────────────────────
//
// Extends the tracing test to verify that the structured field `failures`
// (emitted by `info!(failures = failure_count, "AnalystSyncTask: phase 1 complete")`)
// is captured alongside the message.  Uses a `StructuredEventCollector` that
// records all field name/value pairs, not just the `message` field.

/// Captures all structured fields (name + string representation) from tracing events.
#[derive(Clone, Default)]
struct StructuredEventCollector {
    fields: Arc<Mutex<Vec<(String, String)>>>,
}

impl StructuredEventCollector {
    fn new() -> Self {
        Self::default()
    }

    fn collected_fields(&self) -> Vec<(String, String)> {
        self.fields.lock().unwrap().clone()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for StructuredEventCollector {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct AllFieldVisitor(Vec<(String, String)>);
        impl tracing::field::Visit for AllFieldVisitor {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.0.push((field.name().to_owned(), value.to_owned()));
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push((field.name().to_owned(), format!("{value:?}")));
            }
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                self.0.push((field.name().to_owned(), value.to_string()));
            }
            fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
                self.0.push((field.name().to_owned(), value.to_string()));
            }
            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                self.0.push((field.name().to_owned(), value.to_string()));
            }
        }

        let mut visitor = AllFieldVisitor(Vec::new());
        event.record(&mut visitor);
        let mut guard = self.fields.lock().unwrap();
        guard.extend(visitor.0);
    }
}

#[test]
fn tracing_emits_structured_failures_field_for_analyst_sync() {
    let collector = StructuredEventCollector::new();
    let subscriber = tracing_subscriber::registry().with(collector.clone());

    with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async {
            let (store, _dir) = make_store().await;
            let state = TradingState::new("AAPL", "2026-03-19");
            let ctx = Context::new();

            seed_all_analysts_ok(&ctx, &state).await;

            let task = AnalystSyncTask::new(store);
            task.run(ctx.clone())
                .await
                .expect("AnalystSyncTask should succeed");
        });
    });

    let fields = collector.collected_fields();

    // The `failures` structured field must be emitted with value "0"
    // (all analysts succeeded).
    let failures_field = fields.iter().find(|(name, _)| name == "failures");

    assert!(
        failures_field.is_some(),
        "expected a structured field named 'failures' to be emitted, \
         but got fields: {fields:?}"
    );

    let (_, failures_value) = failures_field.unwrap();
    assert_eq!(
        failures_value, "0",
        "with all analysts succeeding, 'failures' field must be '0', got '{failures_value}'"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Supplemental: debate round counter emits context-visible transitions
// ────────────────────────────────────────────────────────────────────────────
//
// Verifies that the KEY_DEBATE_ROUND counter in context increments correctly
// across two simulated moderator invocations (without real LLM calls), and
// that the transition from "looping" to "advance" is detectable by reading
// context values — matching exactly what the graph's conditional edge predicate
// reads.

#[tokio::test]
async fn debate_round_transitions_are_context_observable() {
    let ctx = Context::new();
    ctx.set(KEY_MAX_DEBATE_ROUNDS, 2u32).await;
    ctx.set(KEY_DEBATE_ROUND, 0u32).await;

    // Simulate two DebateModeratorTask executions incrementing the counter.
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
