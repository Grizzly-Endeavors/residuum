//! Communication channels between the user and the agent.

pub mod cli;
pub mod null;

/// A message from the user to the agent.
#[derive(Debug, Clone)]
pub struct UserMessage {
    /// The text content of the user's message.
    pub content: String,
}

/// A response from the agent to display to the user.
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// The text content of the agent's response.
    pub content: String,
}

/// Trait for displaying agent tool activity during a turn.
///
/// Implemented by `CliDisplay` for interactive sessions and `NullDisplay`
/// for background pulse/cron turns that run without user interaction.
pub trait TurnDisplay: Send + Sync {
    /// Display a tool call being made.
    fn show_tool_call(&self, name: &str, args: &serde_json::Value);

    /// Display the result of a tool call.
    fn show_tool_result(&self, name: &str, output: &str, is_error: bool);
}
