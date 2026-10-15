//! What every workspace crate's tests share for waiting on what they check,
//! behind the `test-support` feature (and this crate's own `cfg(test)`).
//!
//! A test waits on the event it checks, never on elapsed time: a duration that
//! holds on an idle machine fails when several builds share it. Everything
//! here waits on a signal, under one hang guard ([`wait::HANG_GUARD`]) that
//! only stops a test that would otherwise hang.
//!
//! - [`wait`]: waits on channels, watches and polled conditions.
//! - [`clock`]: moving a paused clock, for code with no real I/O.
//! - [`gate`]: holding a call open until the test lets it through.
//! - [`log`]: capturing the log events a test causes.
//! - [`ports`]: loopback ports for a test that tells a component which port to
//!   bind.
//! - [`script`]: writing a fake tool the test then runs, so its exec never
//!   fails with "Text file busy" when another test forks.
//!
//! `crypto.rs` in this directory is not a module of this crate, which builds
//! no HTTP client. Each test binary that can build one includes it by path
//! (`#[path = "…/crates/core/src/testing/crypto.rs"] mod crypto;`), so the
//! rustls provider it installs before `main` is linked into that binary.
//!
//! The root crate's `testing::source_scan` keeps tests on these helpers: test
//! code may not call `sleep`, `timeout` or wiremock's `set_delay` outside
//! [`wait`] and [`clock`], and may not make a file executable outside
//! [`script`]. `CONTRIBUTING.md` ("Waiting in tests") describes which wait
//! fits which situation.

pub mod clock;
pub mod gate;
pub mod log;
pub mod ports;
pub mod script;
pub mod wait;
