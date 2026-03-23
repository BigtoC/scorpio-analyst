#![cfg(feature = "test-helpers")]

use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct EventCollector {
    events: Arc<Mutex<Vec<String>>>,
}

impl EventCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collected(&self) -> Vec<String> {
        self.events.lock().expect("collector lock").clone()
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
                    self.0 = format!("{value:?}");
                }
            }
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if !visitor.0.is_empty() {
            self.events.lock().expect("collector lock").push(visitor.0);
        }
    }
}

#[derive(Clone, Default)]
pub struct StructuredEventCollector {
    fields: Arc<Mutex<Vec<(String, String)>>>,
}

impl StructuredEventCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collected_fields(&self) -> Vec<(String, String)> {
        self.fields.lock().expect("collector lock").clone()
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
        self.fields
            .lock()
            .expect("collector lock")
            .extend(visitor.0);
    }
}
