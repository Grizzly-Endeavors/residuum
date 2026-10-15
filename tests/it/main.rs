//! The crate's integration tests, built as one test binary.
//!
//! Every file directly in `tests/` is a binary of its own that links the whole
//! crate, so the integration tests live in this one instead: a new test file
//! goes in `tests/it/` and gets a `mod` line below. `tests/ts_export.rs` stays
//! a target of its own because it writes the web app's generated types and the
//! scripts that export them run it by name (`cargo test --test ts_export`).
//!
//! The modules share one process. A test leaves process-wide state alone: it
//! doesn't change environment variables or the current directory and doesn't
//! install a global tracing subscriber, because every other module's tests run
//! alongside it.

mod background;
mod daemon;
mod feedback_submit;
mod file_attachment;
mod gateway;
mod inbox;
mod macos_notifications;
mod mcp;
mod memory;
mod proactivity;
mod skills;
mod smoke;
mod subconscious;
mod support;
