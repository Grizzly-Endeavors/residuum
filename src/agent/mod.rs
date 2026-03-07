//! Agent runtime: context assembly, tool loop, and message history management.

pub mod context;
pub mod interrupt;
pub mod recent_messages;
pub(crate) mod turn;
mod core;

pub use core::{Agent, AgentConfig, SystemTurnResult};
