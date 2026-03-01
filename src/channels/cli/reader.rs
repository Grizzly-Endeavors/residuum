//! CLI channel using rustyline for interactive input.

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::error::ResiduumError;

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
    /// Returns `ResiduumError::Channel` if the readline editor cannot be initialized.
    pub fn new() -> Result<Self, ResiduumError> {
        let editor = DefaultEditor::new()
            .map_err(|e| ResiduumError::Channel(format!("failed to initialize readline: {e}")))?;
        Ok(Self { editor })
    }

    /// Read lines from stdin and send them through `tx`.
    ///
    /// After each line is sent, blocks on `gate_rx` until the main loop
    /// signals that the prompt should reappear (after a turn completes or
    /// a slash command is handled). This prevents the prompt from appearing
    /// while the agent is still responding.
    ///
    /// Exits when EOF, `:q`, or `:quit` is entered, or when `tx` is closed.
    /// Ctrl+C cancels the current line and prompts again.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Sender must be owned so dropping it when this fn returns closes the channel"
    )]
    pub fn run(
        mut self,
        tx: tokio::sync::mpsc::Sender<String>,
        prompt: String,
        gate_rx: std::sync::mpsc::Receiver<()>,
    ) {
        loop {
            match self.editor.readline(&prompt) {
                Ok(line) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if matches!(trimmed.as_str(), ":q" | ":quit" | "/quit" | "/exit" | "/q") {
                        return;
                    }
                    if tx.blocking_send(trimmed).is_err() {
                        return; // main task exited
                    }
                    // Wait for the main loop to signal that the turn is done
                    if gate_rx.recv().is_err() {
                        return; // main task dropped the sender
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
