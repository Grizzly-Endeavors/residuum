//! Agent runtime: context assembly, tool loop, and message history management.

pub mod context;
mod core;
pub mod interrupt;
pub mod recent_messages;
pub(crate) mod turn;

pub use core::{Agent, AgentConfig, SystemTurnResult};
