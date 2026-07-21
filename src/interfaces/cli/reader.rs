//! CLI interface using rustyline for interactive input.

use std::path::PathBuf;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use anyhow::Context;

use crate::config::Config;

/// Name of the input history file within the config directory.
const HISTORY_FILE_NAME: &str = "cli_history";

/// Reads user input interactively using rustyline.
///
/// Designed to be moved into a `tokio::task::spawn_blocking` call.
/// Sends input lines through a channel; dropping the sender signals EOF.
pub struct CliReader {
    editor: DefaultEditor,
    /// Where input history is loaded from and saved to. `None` if the config
    /// directory couldn't be resolved, in which case history just isn't persisted.
    history_path: Option<PathBuf>,
}

impl CliReader {
    /// Create a new `CliReader`.
    ///
    /// Loads prior input history from `~/.residuum/cli_history` if present, so
    /// up-arrow recall works across sessions. A missing history file (e.g. on
    /// first run) is expected and not treated as an error.
    ///
    /// # Errors
    /// Returns an error if the readline editor cannot be initialized.
    pub fn new() -> anyhow::Result<Self> {
        let mut editor = DefaultEditor::new().context("failed to initialize readline")?;

        let history_path = match Config::config_dir() {
            Ok(dir) => Some(dir.join(HISTORY_FILE_NAME)),
            Err(e) => {
                tracing::warn!(error = %e, "could not resolve config directory, CLI history will not persist");
                None
            }
        };

        if let Some(path) = &history_path
            && let Err(ReadlineError::Io(e)) = editor.load_history(path)
        {
            if e.kind() == std::io::ErrorKind::NotFound {
                tracing::debug!(path = %path.display(), "no CLI history file yet, starting fresh");
            } else {
                tracing::warn!(error = %e, path = %path.display(), "failed to load CLI history");
            }
        }

        Ok(Self {
            editor,
            history_path,
        })
    }

    /// Save input history to disk, logging (not failing) on error.
    fn save_history(&mut self) {
        if let Some(path) = &self.history_path
            && let Err(e) = self.editor.save_history(path)
        {
            tracing::warn!(error = %e, path = %path.display(), "failed to save CLI history");
        }
    }

    /// Read lines from stdin and send them through `tx`.
    ///
    /// After each line is sent, blocks on `gate_rx` until the main loop
    /// signals that the prompt should reappear (after a turn completes or
    /// a slash command is handled). This prevents the prompt from appearing
    /// while the agent is still responding.
    ///
    /// Exits when EOF, `:q`, `:quit`, or a `/quit` command is processed, or when `tx` is closed.
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
                    if matches!(trimmed.as_str(), ":q" | ":quit") {
                        self.save_history();
                        return;
                    }
                    if tx.blocking_send(trimmed).is_err() {
                        self.save_history();
                        return; // main task exited
                    }
                    // Wait for the main loop to signal that the turn is done
                    if gate_rx.recv().is_err() {
                        self.save_history();
                        return; // main task dropped the sender
                    }
                }
                Err(ReadlineError::Eof) => {
                    self.save_history();
                    return;
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C: cancel current input, prompt again
                }
                Err(e) => {
                    tracing::error!(error = %e, "readline error, exiting input loop");
                    self.save_history();
                    return;
                }
            }
        }
    }
}
