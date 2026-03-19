//! Agent registry for managing multiple independent agent instances.
//!
//! Each named agent gets its own config directory, workspace, port, and PID file
//! under `~/.residuum/agent_registry/<name>/`. The registry tracks agent names
//! and their assigned ports in a TOML file.

pub mod commands;
pub mod paths;
pub mod registry;
