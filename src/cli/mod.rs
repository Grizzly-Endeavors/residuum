//! CLI client with colored output, working indicator, and slash commands.

pub mod colors;
pub mod commands;
pub mod indicator;
pub mod render;

use crate::gateway::protocol::ServerMessage;
use colors::Theme;
use commands::{CommandAction, SlashCommand};
use indicator::WorkingIndicator;
use render::MarkdownRenderer;

/// CLI client that owns the theme, renderer, indicator, and connection state.
pub struct CliClient {
    theme: Theme,
    renderer: MarkdownRenderer,
    /// Working indicator shown during agent turns.
    pub indicator: WorkingIndicator,
    url: String,
    verbose: bool,
}

impl CliClient {
    /// Create a new CLI client for the given gateway URL.
    #[must_use]
    pub fn new(url: impl Into<String>, verbose: bool) -> Self {
        let theme = Theme::detect();
        let renderer = MarkdownRenderer::new(theme.color_enabled());
        Self {
            theme,
            renderer,
            indicator: WorkingIndicator::new(),
            url: url.into(),
            verbose,
        }
    }

    /// Whether verbose mode is currently enabled.
    #[must_use]
    pub fn verbose(&self) -> bool {
        self.verbose
    }

    /// Set verbose mode.
    pub fn set_verbose(&mut self, enabled: bool) {
        self.verbose = enabled;
    }

    /// Get the formatted user input prompt string.
    #[must_use]
    pub fn user_prompt(&self) -> String {
        self.theme.format_user_prompt()
    }

    /// Print the startup banner to stderr.
    pub fn print_banner(&self) {
        let banner = format!("ironclaw v0.1.0 \u{2014} connected to {}", self.url);
        eprintln!("{}", self.theme.format_banner(&banner));
    }

    /// Display a server message with appropriate formatting.
    pub fn display(&mut self, msg: &ServerMessage) {
        match msg {
            ServerMessage::TurnStarted { .. } => {
                self.indicator.start();
            }
            ServerMessage::ToolCall { name, arguments } => {
                self.indicator.on_tool_call();
                if self.verbose {
                    let line = format!("[tool: {name}] {arguments}");
                    eprintln!("{}", self.theme.format_tool(&line));
                }
            }
            ServerMessage::ToolResult {
                name,
                output,
                is_error,
            } => {
                if self.verbose {
                    if *is_error {
                        let line = format!("[tool: {name} ERROR] {output}");
                        eprintln!("{}", self.theme.format_error(&line));
                    } else {
                        let preview = truncate_preview(output, 200);
                        let line = if preview.len() < output.len() {
                            format!("[tool: {name}] {preview}... ({} bytes)", output.len())
                        } else {
                            format!("[tool: {name}] {preview}")
                        };
                        eprintln!("{}", self.theme.format_tool(&line));
                    }
                }
            }
            ServerMessage::Response { content, .. } => {
                self.indicator.finish();
                let prefix = self.theme.format_prefix("IronClaw:");
                let rendered = self.renderer.render(content);
                println!("\n{prefix}\n{rendered}");
            }
            ServerMessage::SystemEvent { source, content } => {
                self.indicator.finish();
                let line = format!("[{source}] {content}");
                println!("\n{}\n", self.theme.format_system_event(&line));
            }
            ServerMessage::Error { message, .. } => {
                self.indicator.finish();
                let line = format!("error: {message}");
                eprintln!("{}", self.theme.format_error(&line));
            }
            ServerMessage::Notice { message } => {
                self.indicator.finish();
                println!("\n{}\n", self.theme.format_notice(message));
            }
            // Reloading is intercepted in run_connect before display() is called
            ServerMessage::Pong | ServerMessage::Reloading => {}
        }
    }

    /// Handle a parsed slash command, returning the action for the main loop.
    #[must_use]
    pub fn handle_command(&self, cmd: &SlashCommand) -> CommandAction {
        cmd.execute(&self.url, self.verbose)
    }
}

/// Truncate a string to at most `max_chars` characters, safe for multi-byte UTF-8.
fn truncate_preview(s: &str, max_chars: usize) -> &str {
    s.char_indices()
        .nth(max_chars)
        .map_or(s, |(idx, _)| s.get(..idx).unwrap_or(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_client_has_correct_defaults() {
        let client = CliClient::new("ws://localhost:7700/ws", false);
        assert!(!client.verbose(), "should start with verbose off");
        assert!(
            !client.indicator.is_active(),
            "indicator should start inactive"
        );
    }

    #[test]
    fn set_verbose_toggles() {
        let mut client = CliClient::new("ws://localhost:7700/ws", false);
        assert!(!client.verbose(), "should start off");
        client.set_verbose(true);
        assert!(client.verbose(), "should be on after set_verbose(true)");
    }

    #[test]
    fn handle_command_routes_correctly() {
        let client = CliClient::new("ws://localhost:7700/ws", true);
        assert_eq!(
            client.handle_command(&SlashCommand::Quit),
            CommandAction::Quit,
            "quit command should return Quit action"
        );
        assert_eq!(
            client.handle_command(&SlashCommand::Verbose),
            CommandAction::ToggleVerbose,
            "verbose command should return ToggleVerbose action"
        );
    }

    #[test]
    fn truncate_preview_ascii() {
        let s = "hello world";
        assert_eq!(
            truncate_preview(s, 5),
            "hello",
            "should truncate at 5 chars"
        );
        assert_eq!(
            truncate_preview(s, 100),
            s,
            "should return full string when under limit"
        );
    }

    #[test]
    fn truncate_preview_multibyte() {
        let s = "\u{1f600}\u{1f600}\u{1f600}abc";
        let result = truncate_preview(s, 3);
        assert_eq!(
            result, "\u{1f600}\u{1f600}\u{1f600}",
            "should truncate at char boundary, not byte boundary"
        );
    }
}
