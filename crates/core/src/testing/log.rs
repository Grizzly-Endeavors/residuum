//! Capturing the log events a test causes, so the test can check what was
//! logged.

use std::sync::Arc;

use tracing_subscriber::layer::SubscriberExt as _;

/// One log event: its level and every field rendered as `name=value`.
#[derive(Debug, Clone)]
pub struct LoggedEvent {
    pub level: tracing::Level,
    pub text: String,
}

/// A tracing layer that records every event, so a test can check what was logged.
#[derive(Clone, Default)]
pub struct EventLog {
    events: Arc<std::sync::Mutex<Vec<LoggedEvent>>>,
}

#[derive(Default)]
struct FieldText(Vec<String>);

impl tracing::field::Visit for FieldText {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test-support helper: a poisoned log lock means a test thread already panicked"
)]
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
    #[must_use]
    pub fn capture(&self) -> tracing::subscriber::DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }

    /// Every event logged so far.
    ///
    /// # Panics
    /// Panics when the log's lock is poisoned, which takes a test thread that
    /// already panicked while logging.
    #[must_use]
    #[expect(
        clippy::unwrap_used,
        reason = "test-support helper: a poisoned log lock means a test thread already panicked"
    )]
    pub fn events(&self) -> Vec<LoggedEvent> {
        self.events.lock().unwrap().clone()
    }

    /// The events logged so far whose text contains `needle`.
    ///
    /// # Panics
    /// Panics when the log's lock is poisoned, which takes a test thread that
    /// already panicked while logging.
    #[must_use]
    pub fn matching(&self, needle: &str) -> Vec<LoggedEvent> {
        self.events()
            .into_iter()
            .filter(|event| event.text.contains(needle))
            .collect()
    }
}
