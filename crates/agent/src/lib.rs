//! Agent runtime: context assembly, tool loop, and message history management.

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod auto_mode;
pub mod context;
mod core;
pub mod hop;
pub mod recent_messages;
mod stream;
pub mod subconscious;
mod team_roster;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod think_tags;
mod tool;
pub mod turn;
#[cfg(test)]
mod turn_conversation_tests;
pub mod usage;

pub use core::{Agent, AgentConfig};
pub use hop::HopCounter;
pub use team_roster::TeamRoster;
pub use tool::{Tool, ToolRegistry};
pub use turn::{LiveOutputEndpoint, ReplyAudience, TurnOutput};
