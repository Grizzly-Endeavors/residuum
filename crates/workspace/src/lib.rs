//! Workspace management: identity files, bootstrapping, the change feed, and
//! the inbox.

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod access;
pub mod bootstrap;
pub mod config;
pub mod identity;
pub mod inbox;
pub mod mcp_patch;
pub mod team;
pub mod team_files;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod version;
pub mod watch;
