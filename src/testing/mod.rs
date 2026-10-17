//! The root crate's own test helpers and the rules its tests enforce on the
//! workspace.
//!
//! The waits, gates, the paused clock, log capture, test ports and fake-tool
//! scripts every crate shares live in `residuum_core::testing` (its
//! `test-support` feature), and the bus's waits on what it publishes live in
//! `residuum_bus::testing`. A test waits on the event it checks, never on
//! elapsed time, under one hang guard.
//!
//! `source_scan` holds the tests that keep it that way: test code may not
//! call `sleep`, `timeout` or wiremock's `set_delay` outside core's `wait`
//! and `clock`, and the waits that predate the rule are counted in
//! `wall_clock_backlog`, where the counts only go down. Test code may not
//! make a file executable itself either; it writes scripts through core's
//! `testing::script`. `CONTRIBUTING.md` ("Waiting in tests") describes which
//! wait fits which situation. `source_scan`'s `unused_pub` rule fails on a
//! member crate's `pub` item that nothing outside that crate's own tests uses,
//! which rustc does not warn about.
//!
//! `manifest_scan` holds the rules the workspace's `Cargo.toml` files keep.
//!
//! A model server and provider that answer through a gate are inference's
//! `testing::model`, reached as `crate::inference::testing::model`.

#[path = "../../crates/core/src/testing/crypto.rs"]
mod crypto;
mod manifest_scan;
mod source_scan;
mod wall_clock_backlog;
