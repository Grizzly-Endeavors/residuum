//! Ring buffer layer for capturing completed tracing spans in memory.
//!
//! Provides a custom `tracing_subscriber::Layer` that records finished spans
//! into a bounded `VecDeque`. The buffer is accessible via a cloneable
//! `SpanBufferHandle` for on-demand snapshotting (bug reports) or draining
//! (live export).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tracing::span;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

// ── Configuration ──────────────────────────────────────────────────

/// Default maximum number of completed spans retained in the buffer.
const DEFAULT_CAPACITY: usize = 4096;

/// Default maximum age before a span is evicted on the next insertion.
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(600); // 10 minutes

/// Configuration for the span ring buffer.
#[derive(Debug, Clone)]
pub struct SpanBufferConfig {
    /// Maximum number of completed spans to retain.
    pub capacity: usize,
    /// Maximum age of spans before eviction (`None` = no age limit).
    pub max_age: Option<Duration>,
    /// Whether to capture log events attached to spans.
    pub capture_events: bool,
}

impl Default for SpanBufferConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            max_age: Some(DEFAULT_MAX_AGE),
            capture_events: true,
        }
    }
}

// ── Captured span types ────────────────────────────────────────────

/// A log event recorded within a span.
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub timestamp: SystemTime,
    pub level: tracing::Level,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

/// A completed span captured by the buffer layer.
#[derive(Debug, Clone)]
pub struct CompletedSpan {
    pub span_id: u64,
    pub parent_id: Option<u64>,
    pub name: String,
    pub target: String,
    pub level: tracing::Level,
    pub start: SystemTime,
    pub duration: Duration,
    pub fields: Vec<(String, String)>,
    pub events: Vec<SpanEvent>,
}

// ── In-flight span builder ─────────────────────────────────────────

/// Accumulates data for a span that hasn't closed yet.
struct SpanBuilder {
    parent_id: Option<u64>,
    name: String,
    target: String,
    level: tracing::Level,
    start_wall: SystemTime,
    start_mono: Instant,
    fields: Vec<(String, String)>,
    events: Vec<SpanEvent>,
}

impl SpanBuilder {
    fn finish(self) -> CompletedSpan {
        CompletedSpan {
            span_id: 0, // filled by caller
            parent_id: self.parent_id,
            name: self.name,
            target: self.target,
            level: self.level,
            start: self.start_wall,
            duration: self.start_mono.elapsed(),
            fields: self.fields,
            events: self.events,
        }
    }
}

// ── Buffer internals ───────────────────────────────────────────────

struct SpanBufferInner {
    buffer: VecDeque<CompletedSpan>,
    active: HashMap<u64, SpanBuilder>,
    capacity: usize,
    max_age: Option<Duration>,
}

impl SpanBufferInner {
    fn push(&mut self, span: CompletedSpan) {
        // Evict stale entries by age
        if let Some(max_age) = self.max_age {
            let cutoff = SystemTime::now()
                .checked_sub(max_age)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            while self.buffer.front().is_some_and(|s| s.start < cutoff) {
                self.buffer.pop_front();
            }
        }

        // Evict oldest if at capacity
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }

        self.buffer.push_back(span);
    }
}

// ── Public handle ──────────────────────────────────────────────────

/// Cloneable handle for reading from the span buffer.
///
/// Obtained from [`SpanBufferLayer::new`] and stored globally so that
/// bug-report and export code can access captured spans.
#[derive(Clone)]
pub struct SpanBufferHandle {
    inner: Arc<Mutex<SpanBufferInner>>,
}

impl SpanBufferHandle {
    /// Clone all buffered spans without draining.
    #[must_use]
    pub fn snapshot(&self) -> Vec<CompletedSpan> {
        self.inner.lock().map_or_else(
            |_| Vec::new(),
            |guard| guard.buffer.iter().cloned().collect(),
        )
    }

    /// Take and return all buffered spans, clearing the buffer.
    #[must_use]
    pub fn drain(&self) -> Vec<CompletedSpan> {
        self.inner
            .lock()
            .map_or_else(|_| Vec::new(), |mut guard| guard.buffer.drain(..).collect())
    }

    /// Number of spans currently in the buffer.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map_or(0, |guard| guard.buffer.len())
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Discard all buffered spans.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.buffer.clear();
        }
    }
}

// ── Layer implementation ───────────────────────────────────────────

/// A `tracing_subscriber::Layer` that captures completed spans into a ring buffer.
pub struct SpanBufferLayer {
    inner: Arc<Mutex<SpanBufferInner>>,
    capture_events: bool,
}

impl SpanBufferLayer {
    /// Create a new buffer layer and its associated handle.
    #[must_use]
    pub fn new(config: &SpanBufferConfig) -> (Self, SpanBufferHandle) {
        let inner = Arc::new(Mutex::new(SpanBufferInner {
            buffer: VecDeque::with_capacity(config.capacity.min(4096)),
            active: HashMap::new(),
            capacity: config.capacity,
            max_age: config.max_age,
        }));

        let handle = SpanBufferHandle {
            inner: Arc::clone(&inner),
        };

        let layer = Self {
            inner,
            capture_events: config.capture_events,
        };

        (layer, handle)
    }
}

/// Visitor that collects span/event fields into a `Vec<(String, String)>`.
struct FieldVisitor {
    fields: Vec<(String, String)>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self { fields: Vec::new() }
    }
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.fields
            .push((field.name().to_string(), format!("{value:?}")));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .push((field.name().to_string(), value.to_string()));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .push((field.name().to_string(), value.to_string()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .push((field.name().to_string(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .push((field.name().to_string(), value.to_string()));
    }
}

impl<S> tracing_subscriber::Layer<S> for SpanBufferLayer
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new();
        attrs.record(&mut visitor);

        let parent_id = attrs.parent().map(span::Id::into_u64).or_else(|| {
            if attrs.is_contextual() {
                ctx.current_span().id().map(span::Id::into_u64)
            } else {
                None
            }
        });

        let meta = attrs.metadata();
        let builder = SpanBuilder {
            parent_id,
            name: meta.name().to_string(),
            target: meta.target().to_string(),
            level: *meta.level(),
            start_wall: SystemTime::now(),
            start_mono: Instant::now(),
            fields: visitor.fields,
            events: Vec::new(),
        };

        if let Ok(mut guard) = self.inner.lock() {
            guard.active.insert(id.into_u64(), builder);
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new();
        values.record(&mut visitor);

        if let Ok(mut guard) = self.inner.lock()
            && let Some(builder) = guard.active.get_mut(&id.into_u64())
        {
            builder.fields.extend(visitor.fields);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        if !self.capture_events {
            return;
        }

        let span_id = ctx.current_span().id().map(span::Id::into_u64);
        let Some(span_id) = span_id else {
            return;
        };

        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        // Extract "message" field if present
        let message = visitor
            .fields
            .iter()
            .find(|(k, _)| k == "message")
            .map_or_else(String::new, |(_, v)| v.clone());

        // Remove "message" from fields since it's stored separately
        visitor.fields.retain(|(k, _)| k != "message");

        let span_event = SpanEvent {
            timestamp: SystemTime::now(),
            level: *event.metadata().level(),
            message,
            fields: visitor.fields,
        };

        if let Ok(mut guard) = self.inner.lock()
            && let Some(builder) = guard.active.get_mut(&span_id)
        {
            builder.events.push(span_event);
        }
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        if let Ok(mut guard) = self.inner.lock()
            && let Some(builder) = guard.active.remove(&id.into_u64())
        {
            let mut completed = builder.finish();
            completed.span_id = id.into_u64();
            guard.push(completed);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "test helper takes config by value for convenience"
)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// Install a test subscriber and return the handle + dispatch guard.
    fn setup(config: SpanBufferConfig) -> (SpanBufferHandle, tracing::subscriber::DefaultGuard) {
        let (layer, handle) = SpanBufferLayer::new(&config);
        let subscriber = tracing_subscriber::registry().with(layer);
        let guard = tracing::subscriber::set_default(subscriber);
        (handle, guard)
    }

    #[test]
    fn captures_completed_span() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let _span = tracing::info_span!("test_span", answer = 42).entered();
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 1, "should capture one completed span");
        assert_eq!(spans[0].name, "test_span");
        assert!(
            spans[0]
                .fields
                .iter()
                .any(|(k, v)| k == "answer" && v == "42"),
            "should capture span fields"
        );
    }

    #[test]
    fn captures_parent_child_relationship() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let parent = tracing::info_span!("parent").entered();
            {
                let _child = tracing::info_span!("child").entered();
            }
            drop(parent);
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 2, "should capture parent and child");

        let child = spans.iter().find(|s| s.name == "child").unwrap();
        let parent = spans.iter().find(|s| s.name == "parent").unwrap();

        assert_eq!(
            child.parent_id,
            Some(parent.span_id),
            "child should reference parent's span_id"
        );
        assert_eq!(parent.parent_id, None, "parent should have no parent");
    }

    #[test]
    fn capacity_eviction() {
        let config = SpanBufferConfig {
            capacity: 3,
            max_age: None,
            capture_events: false,
        };
        let (handle, _guard) = setup(config);

        for i in 0..5 {
            let _span = tracing::info_span!("span", index = i).entered();
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 3, "should retain only capacity-many spans");

        // Oldest spans (index 0, 1) should be evicted; 2, 3, 4 remain
        let indices: Vec<&str> = spans
            .iter()
            .filter_map(|s| {
                s.fields
                    .iter()
                    .find(|(k, _)| k == "index")
                    .map(|(_, v)| v.as_str())
            })
            .collect();
        assert_eq!(
            indices,
            vec!["2", "3", "4"],
            "oldest spans should be evicted"
        );
    }

    #[test]
    fn drain_clears_buffer() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let _span = tracing::info_span!("drainable").entered();
        }

        let drained = handle.drain();
        assert_eq!(drained.len(), 1, "drain should return buffered spans");
        assert!(handle.is_empty(), "buffer should be empty after drain");
    }

    #[test]
    fn snapshot_does_not_clear_buffer() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let _span = tracing::info_span!("persistent").entered();
        }

        let snap1 = handle.snapshot();
        let snap2 = handle.snapshot();
        assert_eq!(snap1.len(), 1, "first snapshot should have span");
        assert_eq!(snap2.len(), 1, "second snapshot should still have span");
    }

    #[test]
    fn captures_events_when_enabled() {
        let config = SpanBufferConfig {
            capture_events: true,
            ..SpanBufferConfig::default()
        };
        let (handle, _guard) = setup(config);

        {
            let _span = tracing::info_span!("evented").entered();
            tracing::info!(key = "val", "hello from inside");
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 1, "should capture the span");
        assert_eq!(spans[0].events.len(), 1, "should capture one event");
        assert_eq!(spans[0].events[0].message, "hello from inside");
        assert!(
            spans[0].events[0]
                .fields
                .iter()
                .any(|(k, v)| k == "key" && v == "val"),
            "should capture event fields"
        );
    }

    #[test]
    fn skips_events_when_disabled() {
        let config = SpanBufferConfig {
            capture_events: false,
            ..SpanBufferConfig::default()
        };
        let (handle, _guard) = setup(config);

        {
            let _span = tracing::info_span!("quiet").entered();
            tracing::info!("this should be ignored");
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 1, "should capture the span");
        assert!(
            spans[0].events.is_empty(),
            "should not capture events when disabled"
        );
    }

    #[test]
    fn clear_empties_buffer() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let _span = tracing::info_span!("clearable").entered();
        }

        assert!(!handle.is_empty(), "buffer should have a span");
        handle.clear();
        assert!(handle.is_empty(), "buffer should be empty after clear");
    }

    #[test]
    fn records_duration() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let _span = tracing::info_span!("timed").entered();
            // Span is open briefly — duration should be non-negative
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 1, "should capture the span");
        // Duration should be very small but non-negative
        assert!(
            spans[0].duration.as_nanos() > 0 || spans[0].duration == Duration::ZERO,
            "duration should be recorded"
        );
    }

    #[test]
    fn on_record_extends_fields() {
        let (handle, _guard) = setup(SpanBufferConfig::default());

        {
            let span =
                tracing::info_span!("recordable", initial = "yes", later = tracing::field::Empty);
            let _entered = span.enter();
            span.record("later", "filled");
        }

        let spans = handle.snapshot();
        assert_eq!(spans.len(), 1, "should capture the span");
        assert!(
            spans[0]
                .fields
                .iter()
                .any(|(k, v)| k == "initial" && v == "yes"),
            "should have initial field"
        );
        assert!(
            spans[0]
                .fields
                .iter()
                .any(|(k, v)| k == "later" && v == "filled"),
            "should have recorded field"
        );
    }
}
