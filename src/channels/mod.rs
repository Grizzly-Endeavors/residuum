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

/// Trait for displaying agent tool activity during a turn.
///
/// Implemented by `BroadcastDisplay` for gateway use and `NullDisplay`
/// for background pulse/cron turns that run without user interaction.
pub trait TurnDisplay: Send + Sync {
    /// Display a tool call being made.
    fn show_tool_call(&self, name: &str, args: &serde_json::Value);

    /// Display the result of a tool call.
    fn show_tool_result(&self, name: &str, output: &str, is_error: bool);

    /// Display an intermediate text response emitted alongside tool calls.
    fn show_response(&self, content: &str);
}
