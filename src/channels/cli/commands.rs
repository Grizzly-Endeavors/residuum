//! Slash command parsing and dispatch for the CLI client.

/// A parsed slash command from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// Show available commands.
    Help,
    /// Show connection status.
    Status,
    /// Toggle verbose mode.
    Verbose,
    /// Reload the gateway configuration.
    Reload,
    /// Force a memory observation cycle.
    Observe,
    /// Force a reflection cycle.
    Reflect,
    /// Quit the client.
    Quit,
    /// An unrecognized slash command.
    Unknown(String),
}

/// The action the main loop should take after handling a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    /// No action needed (command was fully handled).
    None,
    /// Print output text to the user.
    PrintOutput(String),
    /// Toggle verbose mode on/off.
    ToggleVerbose,
    /// Send a reload request to the server.
    Reload,
    /// Send an observe request to the server.
    ObserveRequest,
    /// Send a reflect request to the server.
    ReflectRequest,
    /// Exit the client.
    Quit,
}

impl SlashCommand {
    /// Parse a line of user input as a slash command.
    ///
    /// Returns `None` if the input does not start with `/`.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        let cmd = trimmed.strip_prefix('/')?;
        let keyword = cmd.split_whitespace().next().unwrap_or(cmd);

        Some(match keyword {
            "help" | "h" => Self::Help,
            "status" => Self::Status,
            "verbose" | "v" => Self::Verbose,
            "reload" | "r" => Self::Reload,
            "observe" | "obs" => Self::Observe,
            "reflect" | "ref" => Self::Reflect,
            "quit" | "exit" | "q" => Self::Quit,
            _ => Self::Unknown(keyword.to_string()),
        })
    }

    /// Execute a command, returning the action for the main loop.
    ///
    /// `url` and `verbose` provide current state for `/status`.
    #[must_use]
    pub fn execute(&self, url: &str, verbose: bool) -> CommandAction {
        match self {
            Self::Help => CommandAction::PrintOutput(help_text()),
            Self::Status => CommandAction::PrintOutput(status_text(url, verbose)),
            Self::Verbose => CommandAction::ToggleVerbose,
            Self::Reload => CommandAction::Reload,
            Self::Observe => CommandAction::ObserveRequest,
            Self::Reflect => CommandAction::ReflectRequest,
            Self::Quit => CommandAction::Quit,
            Self::Unknown(name) => {
                CommandAction::PrintOutput(format!("unknown command: /{name} (try /help)"))
            }
        }
    }
}

fn help_text() -> String {
    [
        "Available commands:",
        "  /help, /h       — show this help",
        "  /status         — show connection info",
        "  /verbose, /v    — toggle verbose mode (tool events)",
        "  /reload, /r     — reload server configuration",
        "  /observe, /obs  — force a memory observation cycle",
        "  /reflect, /ref  — force a reflection cycle",
        "  /quit, /exit, /q — disconnect and exit",
    ]
    .join("\n")
}

fn status_text(url: &str, verbose: bool) -> String {
    let verbose_label = if verbose { "on" } else { "off" };
    [
        format!("connected to: {url}"),
        format!("verbose: {verbose_label}"),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help() {
        assert_eq!(
            SlashCommand::parse("/help"),
            Some(SlashCommand::Help),
            "should parse /help"
        );
        assert_eq!(
            SlashCommand::parse("/h"),
            Some(SlashCommand::Help),
            "should parse /h alias"
        );
    }

    #[test]
    fn parse_status() {
        assert_eq!(
            SlashCommand::parse("/status"),
            Some(SlashCommand::Status),
            "should parse /status"
        );
    }

    #[test]
    fn parse_verbose() {
        assert_eq!(
            SlashCommand::parse("/verbose"),
            Some(SlashCommand::Verbose),
            "should parse /verbose"
        );
        assert_eq!(
            SlashCommand::parse("/v"),
            Some(SlashCommand::Verbose),
            "should parse /v alias"
        );
    }

    #[test]
    fn parse_reload() {
        assert_eq!(
            SlashCommand::parse("/reload"),
            Some(SlashCommand::Reload),
            "should parse /reload"
        );
        assert_eq!(
            SlashCommand::parse("/r"),
            Some(SlashCommand::Reload),
            "should parse /r alias"
        );
    }

    #[test]
    fn parse_observe() {
        assert_eq!(
            SlashCommand::parse("/observe"),
            Some(SlashCommand::Observe),
            "should parse /observe"
        );
        assert_eq!(
            SlashCommand::parse("/obs"),
            Some(SlashCommand::Observe),
            "should parse /obs alias"
        );
    }

    #[test]
    fn parse_reflect() {
        assert_eq!(
            SlashCommand::parse("/reflect"),
            Some(SlashCommand::Reflect),
            "should parse /reflect"
        );
        assert_eq!(
            SlashCommand::parse("/ref"),
            Some(SlashCommand::Reflect),
            "should parse /ref alias"
        );
    }

    #[test]
    fn execute_observe_returns_observe_request() {
        let action = SlashCommand::Observe.execute("ws://localhost:7700/ws", false);
        assert_eq!(
            action,
            CommandAction::ObserveRequest,
            "observe should return ObserveRequest"
        );
    }

    #[test]
    fn execute_reflect_returns_reflect_request() {
        let action = SlashCommand::Reflect.execute("ws://localhost:7700/ws", false);
        assert_eq!(
            action,
            CommandAction::ReflectRequest,
            "reflect should return ReflectRequest"
        );
    }

    #[test]
    fn parse_quit_variants() {
        assert_eq!(
            SlashCommand::parse("/quit"),
            Some(SlashCommand::Quit),
            "should parse /quit"
        );
        assert_eq!(
            SlashCommand::parse("/exit"),
            Some(SlashCommand::Quit),
            "should parse /exit"
        );
        assert_eq!(
            SlashCommand::parse("/q"),
            Some(SlashCommand::Quit),
            "should parse /q"
        );
    }

    #[test]
    fn parse_unknown() {
        assert_eq!(
            SlashCommand::parse("/foobar"),
            Some(SlashCommand::Unknown("foobar".to_string())),
            "should parse unknown command"
        );
    }

    #[test]
    fn parse_non_slash_returns_none() {
        assert_eq!(
            SlashCommand::parse("hello world"),
            None,
            "non-slash input should return None"
        );
    }

    #[test]
    fn parse_with_leading_whitespace() {
        assert_eq!(
            SlashCommand::parse("  /help"),
            Some(SlashCommand::Help),
            "should handle leading whitespace"
        );
    }

    #[test]
    fn execute_help_returns_output() {
        let action = SlashCommand::Help.execute("ws://localhost:7700/ws", false);
        assert!(
            matches!(&action, CommandAction::PrintOutput(text) if text.contains("/help")),
            "help text should be PrintOutput containing /help"
        );
    }

    #[test]
    fn execute_status_shows_url_and_verbose() {
        let action = SlashCommand::Status.execute("ws://localhost:7700/ws", true);
        assert!(
            matches!(
                &action,
                CommandAction::PrintOutput(text)
                    if text.contains("ws://localhost:7700/ws") && text.contains("verbose: on")
            ),
            "status should be PrintOutput containing url and verbose state"
        );
    }

    #[test]
    fn execute_verbose_returns_toggle() {
        let action = SlashCommand::Verbose.execute("ws://localhost:7700/ws", false);
        assert_eq!(
            action,
            CommandAction::ToggleVerbose,
            "verbose should return ToggleVerbose"
        );
    }

    #[test]
    fn execute_reload_returns_reload() {
        let action = SlashCommand::Reload.execute("ws://localhost:7700/ws", false);
        assert_eq!(action, CommandAction::Reload, "reload should return Reload");
    }

    #[test]
    fn execute_quit_returns_quit() {
        let action = SlashCommand::Quit.execute("ws://localhost:7700/ws", false);
        assert_eq!(action, CommandAction::Quit, "quit should return Quit");
    }

    #[test]
    fn execute_unknown_returns_message() {
        let action =
            SlashCommand::Unknown("xyz".to_string()).execute("ws://localhost:7700/ws", false);
        assert!(
            matches!(&action, CommandAction::PrintOutput(text) if text.contains("unknown command")),
            "unknown should be PrintOutput containing 'unknown command'"
        );
    }
}
