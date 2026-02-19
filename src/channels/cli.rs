//! CLI channel using rustyline for interactive input.

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use serde_json::Value;

use super::{AgentResponse, TurnDisplay, UserMessage};
use crate::error::IronclawError;

/// Reads user input interactively using rustyline.
///
/// Designed to be moved into a `tokio::task::spawn_blocking` call.
/// Sends input lines through a channel; dropping the sender signals EOF.
pub struct CliReader {
    editor: DefaultEditor,
}

impl CliReader {
    /// Create a new `CliReader`.
    ///
    /// # Errors
    /// Returns `IronclawError::Channel` if the readline editor cannot be initialized.
    pub fn new() -> Result<Self, IronclawError> {
        let editor = DefaultEditor::new()
            .map_err(|e| IronclawError::Channel(format!("failed to initialize readline: {e}")))?;
        Ok(Self { editor })
    }

    /// Read lines from stdin and send them through `tx`.
    ///
    /// Exits when EOF, `:q`, or `:quit` is entered, or when `tx` is closed.
    /// Ctrl+C cancels the current line and prompts again.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Sender must be owned so dropping it when this fn returns closes the channel"
    )]
    pub fn run(mut self, tx: tokio::sync::mpsc::Sender<String>) {
        loop {
            match self.editor.readline("you> ") {
                Ok(line) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed == ":q" || trimmed == ":quit" {
                        return;
                    }
                    if tx.blocking_send(trimmed).is_err() {
                        return; // main task exited
                    }
                }
                Err(ReadlineError::Eof) => return,
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C: cancel current input, prompt again
                }
                Err(e) => {
                    eprintln!("readline error: {e}");
                    return;
                }
            }
        }
    }
}

/// Displays agent responses and tool activity to the user.
///
/// Implements `TurnDisplay` for use with `Agent::process_message`.
pub struct CliDisplay {
    agent_name: String,
}

impl CliDisplay {
    /// Create a new `CliDisplay` with the given agent name for response prefix.
    #[must_use]
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
        }
    }

    /// Display an agent response to the user.
    pub fn show_response(&self, response: &AgentResponse) {
        println!("{}: {}", self.agent_name, response.content);
    }
}

impl TurnDisplay for CliDisplay {
    fn show_tool_call(&self, name: &str, arguments: &Value) {
        eprintln!("[tool: {name}] {arguments}");
    }

    fn show_tool_result(&self, name: &str, output: &str, is_error: bool) {
        if is_error {
            eprintln!("[tool: {name} ERROR] {output}");
        } else {
            let preview = if output.len() > 200 {
                format!(
                    "{}... ({} bytes)",
                    output.get(..200).unwrap_or(output),
                    output.len()
                )
            } else {
                output.to_string()
            };
            eprintln!("[tool: {name}] {preview}");
        }
    }
}

/// Legacy combined channel; kept for tests that create both reader and display together.
///
/// For new code use `CliReader` + `CliDisplay` separately.
pub struct CliChannel {
    reader: CliReader,
    display: CliDisplay,
}

impl CliChannel {
    /// Create a new CLI channel with the given agent name for display.
    ///
    /// # Errors
    /// Returns `IronclawError::Channel` if the readline editor cannot be initialized.
    pub fn new(agent_name: impl Into<String>) -> Result<Self, IronclawError> {
        Ok(Self {
            reader: CliReader::new()?,
            display: CliDisplay::new(agent_name),
        })
    }

    /// Read a message from the user (synchronous, for tests).
    ///
    /// Returns `None` on EOF (Ctrl+D) or exit commands (`:q`, `:quit`).
    /// Ctrl+C cancels the current input and prompts again.
    ///
    /// # Errors
    /// Returns `IronclawError::Channel` on unexpected readline errors.
    pub fn read_message(&mut self) -> Result<Option<UserMessage>, IronclawError> {
        loop {
            match self.reader.editor.readline("you> ") {
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
                Err(ReadlineError::Interrupted) => {}
                Err(e) => {
                    return Err(IronclawError::Channel(format!("readline error: {e}")));
                }
            }
        }
    }

    /// Display an agent response to the user.
    pub fn show_response(&self, response: &AgentResponse) {
        self.display.show_response(response);
    }

    /// Get a reference to the display component.
    #[must_use]
    pub fn display(&self) -> &CliDisplay {
        &self.display
    }
}

impl TurnDisplay for CliChannel {
    fn show_tool_call(&self, name: &str, args: &serde_json::Value) {
        self.display.show_tool_call(name, args);
    }

    fn show_tool_result(&self, name: &str, output: &str, is_error: bool) {
        self.display.show_tool_result(name, output, is_error);
    }
}
