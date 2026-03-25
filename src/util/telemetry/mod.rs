//! Telemetry infrastructure for span capture and export.
//!
//! Provides a ring buffer layer that records completed tracing spans in memory,
//! accessible via a global handle for on-demand snapshotting (bug reports) or
//! draining (live export to an OTLP-compatible backend).

mod buffer;

pub use buffer::{CompletedSpan, SpanBufferConfig, SpanBufferHandle, SpanBufferLayer, SpanEvent};

use std::sync::OnceLock;

/// Process-global span buffer handle, set once during tracing initialization.
static SPAN_BUFFER: OnceLock<SpanBufferHandle> = OnceLock::new();

/// Access the global span buffer handle.
///
/// Returns `None` if the span buffer layer was not initialized (e.g. in CLI
/// or test modes that don't call `init_daemon_tracing`).
#[must_use]
pub fn global_span_buffer() -> Option<&'static SpanBufferHandle> {
    SPAN_BUFFER.get()
}

/// Set the global span buffer handle. Called once during tracing init.
///
/// Returns `Err` if the handle was already set (only one buffer per process).
///
/// # Errors
///
/// Returns the handle back if a global buffer was already initialized.
pub fn set_global_span_buffer(handle: SpanBufferHandle) -> Result<(), SpanBufferHandle> {
    SPAN_BUFFER.set(handle)
}
