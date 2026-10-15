//! The root crate's own test helpers and the rules its tests enforce on the
//! workspace.
//!
//! The waits, gates, the paused clock, log capture, test ports and fake-tool
//! scripts every crate shares live in `residuum_core::testing` (its
//! `test-support` feature). A test waits on the event it checks, never on
//! elapsed time, under that module's one hang guard. What waits on the root's
//! own types is here:
//!
//! - [`bus_wait`]: waits on what the bus and the hub publish.
//!
//! A model server and provider that answer through a gate are inference's
//! `testing::model`, reached as `crate::inference::testing::model`.
//!
//! `source_scan` holds the tests that keep it that way: test code may not
//! call `sleep`, `timeout` or wiremock's `set_delay` outside core's `wait`
//! and `clock`, and the waits that predate the rule are counted in
//! `wall_clock_backlog`, where the counts only go down. Test code may not
//! make a file executable itself either; it writes scripts through core's
//! `testing::script`. `CONTRIBUTING.md` ("Waiting in tests") describes which
//! wait fits which situation.
//!
//! `manifest_scan` holds the rules the workspace's `Cargo.toml` files keep.

pub(crate) mod bus_wait;
#[path = "../../crates/core/src/testing/crypto.rs"]
mod crypto;
mod manifest_scan;
mod source_scan;
mod wall_clock_backlog;
