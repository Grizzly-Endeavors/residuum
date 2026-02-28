//! Communication channels between the user and the agent.

#[cfg(feature = "discord")]
pub mod attachment;
pub mod chunking;
pub mod cli;
#[cfg(feature = "discord")]
pub mod discord;
pub mod null;
#[cfg(feature = "discord")]
pub mod presence;
pub mod types;
pub mod webhook;
pub mod websocket;
