//! Agent runtime: context assembly, tool loop, and message history management.

pub mod auto_mode;
pub mod context;
mod core;
pub mod hop;
pub mod recent_messages;
mod stream;
mod team_roster;
#[cfg(test)]
pub(crate) mod testing;
mod think_tags;
mod tool;
pub(crate) mod turn;
#[cfg(test)]
mod turn_conversation_tests;
pub mod usage;

pub use core::{Agent, AgentConfig};
pub use hop::HopCounter;
pub use team_roster::TeamRoster;
pub use tool::{Tool, ToolRegistry};
pub use turn::{LiveOutputEndpoint, ReplyAudience, TurnOutput};
