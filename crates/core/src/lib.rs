//! The bottom layer of the Residuum workspace: shared utilities, timezone-aware
//! local time, and the diagnostic types validators report. It depends on no
//! other workspace crate.
//!
//! The `test-support` feature adds `testing`, the waits, gates and clock
//! helpers every crate's tests use.

pub mod build_info;
pub mod diagnostics;
pub mod process;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod time;
pub mod util;

pub use build_info::BuildInfo;
