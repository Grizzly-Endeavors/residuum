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
                let line = format!("[tool: {name}] {arguments}");
                eprintln!("{}", self.theme.format_tool(&line));
            }
            ServerMessage::ToolResult {
                name,
                output,
                is_error,
            } => {
                if *is_error {
                    let line = format!("[tool: {name} ERROR] {output}");
                    eprintln!("{}", self.theme.format_error(&line));
                } else {
                    let preview = if output.len() > 200 {
                        format!(
                            "{}... ({} bytes)",
                            output.get(..200).unwrap_or(output),
                            output.len()
                        )
                    } else {
                        output.clone()
                    };
                    let line = format!("[tool: {name}] {preview}");
                    eprintln!("{}", self.theme.format_tool(&line));
                }
            }
            ServerMessage::Response { content, .. } => {
                self.indicator.finish();
                let prefix = self.theme.format_prefix("ironclaw:");
                let rendered = self.renderer.render(content);
                println!("{prefix} {rendered}");
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
}
