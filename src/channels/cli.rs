//! CLI channel using rustyline for interactive input.

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde_json::Value;

use super::{AgentResponse, UserMessage};
use crate::error::IronclawError;

/// Interactive CLI channel for reading user input and displaying responses.
pub struct CliChannel {
    editor: DefaultEditor,
    agent_name: String,
}

impl CliChannel {
    /// Create a new CLI channel with the given agent name for display.
    ///
    /// # Errors
    /// Returns `IronclawError::Channel` if the readline editor cannot be initialized.
    pub fn new(agent_name: impl Into<String>) -> Result<Self, IronclawError> {
        let editor = DefaultEditor::new()
            .map_err(|e| IronclawError::Channel(format!("failed to initialize readline: {e}")))?;

        Ok(Self {
            editor,
            agent_name: agent_name.into(),
        })
    }

    /// Read a message from the user.
    ///
    /// Returns `None` on EOF (Ctrl+D) or exit commands (`:q`, `:quit`).
    /// Ctrl+C cancels the current input and prompts again.
    ///
    /// # Errors
    /// Returns `IronclawError::Channel` on unexpected readline errors.
    pub fn read_message(&mut self) -> Result<Option<UserMessage>, IronclawError> {
        loop {
            match self.editor.readline("you> ") {
                Ok(line) => {
                    let trimmed = line.trim();

                    if trimmed.is_empty() {
                        continue;
                    }

                    if trimmed == ":q" || trimmed == ":quit" {
                        return Ok(None);
                    }

                    return Ok(Some(UserMessage {
                        content: trimmed.to_string(),
                    }));
                }
                Err(ReadlineError::Eof) => return Ok(None),
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C: cancel current input, prompt again
                }
                Err(e) => {
                    return Err(IronclawError::Channel(format!("readline error: {e}")));
                }
            }
        }
    }

    /// Display an agent response to the user.
    pub fn show_response(&self, response: &AgentResponse) {
        println!("{}: {}", self.agent_name, response.content);
    }

    /// Display a tool call for transparency.
    pub fn show_tool_call(&self, name: &str, arguments: &Value) {
        eprintln!("[tool: {name}] {arguments}");
    }

    /// Display a tool result.
    pub fn show_tool_result(&self, name: &str, output: &str, is_error: bool) {
        if is_error {
            eprintln!("[tool: {name} ERROR] {output}");
        } else {
            let preview = if output.len() > 200 {
                format!(
                    "{}... ({} bytes)",
                    &output.get(..200).unwrap_or(output),
                    output.len()
                )
            } else {
                output.to_string()
            };
            eprintln!("[tool: {name}] {preview}");
        }
    }
}
