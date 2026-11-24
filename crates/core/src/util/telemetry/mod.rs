mod buffer;

pub use buffer::{CompletedSpan, SpanBufferConfig, SpanBufferHandle, SpanBufferLayer, SpanEvent};

use std::sync::OnceLock;

static SPAN_BUFFER: OnceLock<SpanBufferHandle> = OnceLock::new();

#[must_use]
pub(crate) fn global_span_buffer() -> Option<&'static SpanBufferHandle> {
    SPAN_BUFFER.get()
}

pub fn set_global_span_buffer(handle: SpanBufferHandle) -> Result<(), SpanBufferHandle> {
    SPAN_BUFFER.set(handle)
}

#[must_use]
pub fn span_buffer() -> SpanBufferHandle {
    global_span_buffer().cloned().unwrap_or_else(|| {
        let (_layer, handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        handle
    })
}
