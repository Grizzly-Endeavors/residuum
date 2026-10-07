//! Sibling instances of one user: the join that pairs two of them, and the
//! keys that join leaves behind.
//!
//! `docs/systems-usage/remote-access.md` describes the whole flow.

pub(crate) mod client;
pub(crate) mod discovery;
pub(crate) mod host;
pub(crate) mod keys;
pub(crate) mod protocol;
pub(crate) mod routes;
pub(crate) mod service;

pub use keys::{NoSiblings, SiblingKeyVerifier, SiblingKeys};
