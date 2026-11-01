//! What other crates' tests use to drive a model, behind the `test-support`
//! feature (and this crate's own `cfg(test)`).
//!
//! - [`model`]: a model server and provider that answer through a gate.
//! - [`clients`]: the real provider clients, pointed at a test server.

pub mod clients;
pub mod model;
