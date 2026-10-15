//! Capturing the log events a test causes, so the test can check what was
//! logged.

use std::sync::Arc;

use tracing_subscriber::layer::SubscriberExt as _;

/// One log event: its level and every field rendered as `name=value`.
#[derive(Debug, Clone)]
pub(crate) struct LoggedEvent {
    pub(crate) level: tracing::Level,
    pub(crate) text: String,
}

#[derive(Clone, Default)]
pub(crate) struct EventLog {
    pub(crate) events: Arc<std::sync::Mutex<Vec<LoggedEvent>>>,
}

#[derive(Default)]
struct FieldText(Vec<String>);

impl tracing::field::Visit for FieldText {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for EventLog {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = FieldText::default();
        event.record(&mut fields);
        self.events.lock().unwrap().push(LoggedEvent {
            level: *event.metadata().level(),
            text: fields.0.join(" "),
        });
    }
}

impl EventLog {
    /// Route this thread's log events here until the guard drops.
    pub(crate) fn capture(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }

    pub(crate) fn matching(&self, needle: &str) -> Vec<LoggedEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.text.contains(needle))
            .cloned()
            .collect()
    }
}
