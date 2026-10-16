//! Workspace management: identity files, bootstrapping, and the change feed.

pub mod access;
pub mod bootstrap;
pub mod config;
pub mod identity;
pub mod mcp_patch;
pub mod team;
pub mod team_files;
#[cfg(test)]
pub(crate) mod testing;
pub mod version;
pub mod watch;
