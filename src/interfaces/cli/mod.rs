//! CLI client with colored output, working indicator, and slash commands.

pub mod colors;
pub mod commands;
pub mod indicator;
pub mod reader;
pub mod render;

pub use reader::CliReader;

use crate::gateway::protocol::ServerMessage;
use colors::Theme;
use commands::CommandEffect;
use indicator::WorkingIndicator;
use render::MarkdownRenderer;

/// Convert a WebSocket URL to an HTTP URL for display.
///
/// Replaces `ws://` → `http://` and `wss://` → `https://`, and strips a
/// trailing `/ws` path segment if present.
#[must_use]
pub fn ws_url_to_http(ws_url: &str) -> String {
    let url = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    url.strip_suffix("/ws").unwrap_or(&url).to_string()
}

/// CLI client that owns the theme, renderer, indicator, and connection state.
pub struct CliClient {
    theme: Theme,
    renderer: MarkdownRenderer,
    /// Working indicator shown during agent turns.
    pub indicator: WorkingIndicator,
    url: String,
    verbose: bool,
    /// Whether the agent prefix has been printed for the current turn.
    turn_has_header: bool,
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
            turn_has_header: false,
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

    /// Print the startup banner.
    pub fn print_banner(&self) {
        let banner = format!(
            "residuum {} \u{2014} connected to {}",
            env!("RESIDUUM_VERSION"),
            self.url
        );
        println!("{}", self.theme.format_banner(&banner));
        let http_url = ws_url_to_http(&self.url);
        println!("  web UI: {http_url}");
    }

    /// Display a server message with appropriate formatting.
    pub fn display(&mut self, msg: &ServerMessage) {
        match msg {
            ServerMessage::TurnStarted { .. } => {
                self.turn_has_header = false;
                self.indicator.start();
            }
            ServerMessage::ToolCall {
                name, arguments, ..
            } => {
                self.indicator.on_tool_call();
                if self.verbose {
                    let line = format!("[tool: {name}] {arguments}");
                    println!("{}", self.theme.format_tool(&line));
                }
            }
            ServerMessage::ToolResult {
                name,
                output,
                is_error,
                ..
            } => {
                if self.verbose {
                    if *is_error {
                        let line = format!("[tool: {name} ERROR] {output}");
                        println!("{}", self.theme.format_error(&line));
                    } else {
                        let preview = truncate_preview(output, 200);
                        let line = if preview.len() < output.len() {
                            format!("[tool: {name}] {preview}... ({} bytes)", output.len())
                        } else {
                            format!("[tool: {name}] {preview}")
                        };
                        println!("{}", self.theme.format_tool(&line));
                    }
                }
            }
            ServerMessage::BroadcastResponse { content } => {
                self.indicator.clear_line();
                let header = if self.turn_has_header {
                    String::new()
                } else {
                    self.turn_has_header = true;
                    format!("\n{}\n", self.theme.format_prefix("Residuum:"))
                };
                let rendered = self.renderer.render(content);
                println!("{header}{rendered}");
            }
            ServerMessage::Response { content, .. } => {
                self.indicator.finish();
                let header = if self.turn_has_header {
                    String::new()
                } else {
                    format!("\n{}\n", self.theme.format_prefix("Residuum:"))
                };
                self.turn_has_header = false;
                let rendered = self.renderer.render(content);
                println!("{header}{rendered}");
            }
            ServerMessage::Error { message, .. } => {
                self.indicator.finish();
                let line = format!("error: {message}");
                println!("{}", self.theme.format_error(&line));
            }
            ServerMessage::Notice { message } => {
                self.indicator.finish();
                println!("\n{}\n", self.theme.format_notice(message));
            }
            ServerMessage::FileAttachment {
                filename, url, caption, ..
            } => {
                self.indicator.finish();
                let caption_text = caption.as_deref().unwrap_or("");
                let line = if caption_text.is_empty() {
                    format!("[file: {filename}] {url}")
                } else {
                    format!("[file: {filename}] {url} — {caption_text}")
                };
                println!("{}", self.theme.format_tool(&line));
            }
            // Reloading is intercepted in run_connect before display() is called
            ServerMessage::Pong | ServerMessage::Reloading => {}
        }
    }

    /// Parse and execute a slash command, returning the effect for the main loop.
    ///
    /// Returns `None` if the input is not a slash command.
    #[must_use]
    pub fn handle_command(&self, input: &str) -> Option<CommandEffect> {
        commands::parse_command(input, &self.url, self.verbose)
    }
}

/// Truncate a string to at most `max_chars` characters, safe for multi-byte UTF-8.
fn truncate_preview(s: &str, max_chars: usize) -> &str {
    s.char_indices()
        .nth(max_chars)
        .map_or(s, |(idx, _)| s.split_at(idx).0)
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
            client.handle_command("/quit"),
            Some(CommandEffect::Quit),
            "quit command should return Quit effect"
        );
        assert_eq!(
            client.handle_command("/verbose"),
            Some(CommandEffect::ToggleVerbose),
            "verbose command should return ToggleVerbose effect"
        );
        assert_eq!(
            client.handle_command("hello world"),
            None,
            "non-slash input should return None"
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

    #[test]
    fn ws_url_to_http_basic() {
        assert_eq!(
            ws_url_to_http("ws://127.0.0.1:7700/ws"),
            "http://127.0.0.1:7700",
            "should convert ws to http and strip /ws"
        );
    }

    #[test]
    fn ws_url_to_http_secure() {
        assert_eq!(
            ws_url_to_http("wss://example.com/ws"),
            "https://example.com",
            "should convert wss to https and strip /ws"
        );
    }

    #[test]
    fn ws_url_to_http_no_ws_path() {
        assert_eq!(
            ws_url_to_http("ws://localhost:8080/other"),
            "http://localhost:8080/other",
            "should not strip non-/ws paths"
        );
    }

    #[test]
    fn ws_url_to_http_bare() {
        assert_eq!(
            ws_url_to_http("ws://localhost:7700"),
            "http://localhost:7700",
            "should handle URLs without path"
        );
    }

    #[test]
    fn display_turn_started_sets_indicator_active_and_resets_header() {
        let mut client = CliClient::new("ws://localhost:7700/ws", false);
        client.turn_has_header = true;
        client.display(&ServerMessage::TurnStarted {
            reply_to: "c1".into(),
        });
        assert!(
            client.indicator.is_active(),
            "TurnStarted should activate indicator"
        );
        assert!(
            !client.turn_has_header,
            "TurnStarted should reset turn_has_header"
        );
    }

    #[test]
    fn display_response_clears_indicator() {
        let mut client = CliClient::new("ws://localhost:7700/ws", false);
        client.display(&ServerMessage::TurnStarted {
            reply_to: "c1".into(),
        });
        assert!(client.indicator.is_active());
        client.display(&ServerMessage::Response {
            reply_to: "c1".into(),
            content: "done".into(),
        });
        assert!(
            !client.indicator.is_active(),
            "Response should deactivate indicator"
        );
    }

    #[test]
    fn display_broadcast_response_sets_turn_has_header() {
        let mut client = CliClient::new("ws://localhost:7700/ws", false);
        assert!(!client.turn_has_header, "should start without header");
        client.display(&ServerMessage::BroadcastResponse {
            content: "chunk 1".into(),
        });
        assert!(
            client.turn_has_header,
            "first BroadcastResponse should set turn_has_header"
        );
        client.display(&ServerMessage::BroadcastResponse {
            content: "chunk 2".into(),
        });
        assert!(
            client.turn_has_header,
            "subsequent BroadcastResponse should not clear turn_has_header"
        );
    }
}
