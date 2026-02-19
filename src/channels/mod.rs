//! Communication channels between the user and the agent.

pub mod cli;

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
