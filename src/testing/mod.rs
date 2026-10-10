//! What the crate's tests share for waiting on what they check.
//!
//! A test waits on the event it checks, never on elapsed time: a duration that
//! holds on an idle machine fails when several builds share it. Everything
//! here waits on a signal, under one hang guard ([`wait::HANG_GUARD`]) that
//! only stops a test that would otherwise hang.
//!
//! - [`wait`]: waits on channels, watches and polled conditions.
//! - [`clock`]: moving a paused clock, for code with no real I/O.
//! - [`gate`]: holding a call open until the test lets it through.
//! - [`model`]: a model server and provider that answer through a gate.
//!
//! `source_scan` holds the tests that keep it that way: test code may not
//! call `sleep`, `timeout` or wiremock's `set_delay` outside this module, and
//! the waits that predate the rule are counted in `wall_clock_backlog`, where
//! the counts only go down. `CONTRIBUTING.md` ("Waiting in tests") describes
//! which wait fits which situation.

pub(crate) mod clock;
pub(crate) mod gate;
pub(crate) mod model;
mod source_scan;
pub(crate) mod wait;
mod wall_clock_backlog;
