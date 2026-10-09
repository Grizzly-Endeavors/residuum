//! Agent runtime: context assembly, tool loop, and message history management.

pub mod auto_mode;
pub mod context;
mod core;
pub mod hop;
pub mod interrupt;
pub mod recent_messages;
mod stream;
mod think_tags;
pub(crate) mod turn;
#[cfg(test)]
mod turn_conversation_tests;
pub mod usage;

pub use core::{Agent, AgentConfig};
pub use hop::HopCounter;
