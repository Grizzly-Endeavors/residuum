//! Projects context management: structured, scoped context folders.
//!
//! Entry point is [`activation::ProjectState`], which tracks the active project
//! and its loaded context. `src/tools/projects.rs` drives it (via
//! `SharedProjectState`) and orchestrates the side effects of switching
//! projects — MCP server ref-counting, skill rescanning — one layer above this
//! module. See `docs/systems-usage/projects.md` for the authoritative spec of
//! lifecycle and activation behavior.

pub mod activation;
pub mod lifecycle;
pub mod manifest;
pub mod scanner;
pub mod types;
