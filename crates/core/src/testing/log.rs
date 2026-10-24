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

/// Two empty dispatchers kept registered for the whole test process.
///
/// tracing-core resolves a call site's interest against the calling thread's
/// own default alone while exactly one dispatcher is registered. A call site
/// first hit on a thread with no capture would then cache "never" and drop
/// its events for every later capture. Keeping two registered means the
/// interest is always resolved against every live dispatcher.
static RESIDENT_DISPATCHERS: std::sync::OnceLock<[tracing::Dispatch; 2]> =
    std::sync::OnceLock::new();

impl EventLog {
    /// Route this thread's log events here until the guard drops.
    #[must_use]
    pub fn capture(&self) -> tracing::subscriber::DefaultGuard {
        RESIDENT_DISPATCHERS.get_or_init(|| {
            [
                tracing::Dispatch::new(tracing_subscriber::registry()),
                tracing::Dispatch::new(tracing_subscriber::registry()),
            ]
        });
        let guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()));
        // Call sites this thread registered before the capture were cached
        // without it; recompute them now that it is live.
        tracing::callsite::rebuild_interest_cache();
        guard
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

#[cfg(test)]
mod tests {
    use super::EventLog;

    /// One call site shared by the test's threads: its first call registers
    /// it with the dispatchers live on that thread.
    fn emit_probe() {
        tracing::info!("callsite probe");
    }

    #[test]
    fn a_callsite_first_hit_on_another_thread_is_still_captured() {
        let log = EventLog::default();
        let _capture = log.capture();

        // A thread with no capture of its own registers the call site, as a
        // parallel test that never captures logs would.
        std::thread::spawn(emit_probe)
            .join()
            .expect("probe thread should finish");

        emit_probe();
        assert_eq!(
            log.matching("callsite probe").len(),
            1,
            "a call site another thread registered first is still captured here"
        );
    }
}
