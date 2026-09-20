//! Data-driven slash command registry shared across interfaces.
//!
//! Provides a shared command registry used by Discord, Telegram, and any future
//! interfaces. `execute_command` resolves a command by name and returns a
//! `CommandResult` combining response text with any side effect the interface
//! handler must apply.

/// Metadata about any command, for cross-channel registration.
pub struct CommandInfo {
    /// The primary command name (e.g. "help", "observe").
    pub name: &'static str,
    /// Human-readable help text.
    pub help: &'static str,
    /// Whether the command takes a text argument.
    pub takes_arg: bool,
}

/// Context for executing a command from any interface.
#[derive(Default)]
pub struct CommandContext<'a> {
    /// Connection URL (for status display).
    pub url: &'a str,
    /// Whether verbose mode is enabled.
    pub verbose: bool,
}

/// Result of executing a command through the shared registry.
pub struct CommandResult {
    /// Text response to display to the user.
    pub response: String,
    /// Optional side effect the channel handler must apply.
    pub side_effect: Option<CommandSideEffect>,
}

/// Side effects that interface handlers must apply after a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSideEffect {
    /// Reload server configuration.
    Reload,
    /// Add text to the agent's inbox.
    InboxAdd(String),
    /// Dispatch a named server command.
    ServerCommand {
        /// Command name.
        name: &'static str,
        /// Optional argument text.
        args: Option<String>,
    },
    /// Stop the currently running agent turn, if any.
    ///
    /// Kept separate from `ServerCommand` because a stop must reach a turn
    /// while it's running; `ServerCommand` only gets processed between
    /// turns (see `dispatch_stop_request`).
    Stop,
}

struct CommandDef {
    names: &'static [&'static str],
    help: &'static str,
    takes_arg: bool,
    effect: fn(arg: Option<&str>, url: &str, verbose: bool) -> CommandResult,
}

/// Build a `CommandResult` for a command that dispatches a named server command
/// with no arguments and no local state to report.
fn server_command_result(name: &'static str) -> CommandResult {
    CommandResult {
        response: format!("{name} triggered."),
        side_effect: Some(CommandSideEffect::ServerCommand { name, args: None }),
    }
}

static COMMANDS: &[CommandDef] = &[
    CommandDef {
        names: &["help", "h"],
        help: "show this help",
        takes_arg: false,
        effect: |_, _, _| CommandResult {
            response: help_text(),
            side_effect: None,
        },
    },
    CommandDef {
        names: &["status"],
        help: "show connection info",
        takes_arg: false,
        effect: |_, url, verbose| CommandResult {
            response: status_text(url, verbose),
            side_effect: None,
        },
    },
    CommandDef {
        names: &["reload", "r"],
        help: "reload server configuration",
        takes_arg: false,
        effect: |_, _, _| CommandResult {
            response: "Reloading configuration...".to_string(),
            side_effect: Some(CommandSideEffect::Reload),
        },
    },
    CommandDef {
        names: &["observe", "obs"],
        help: "force a memory observation cycle",
        takes_arg: false,
        effect: |_, _, _| server_command_result("observe"),
    },
    CommandDef {
        names: &["reflect", "ref"],
        help: "force a reflection cycle",
        takes_arg: false,
        effect: |_, _, _| server_command_result("reflect"),
    },
    CommandDef {
        names: &["context", "ctx"],
        help: "show context token usage",
        takes_arg: false,
        effect: |_, _, _| server_command_result("context"),
    },
    CommandDef {
        names: &["stop"],
        help: "stop the current agent turn",
        takes_arg: false,
        effect: |_, _, _| CommandResult {
            response: "stopping the current turn…".to_string(),
            side_effect: Some(CommandSideEffect::Stop),
        },
    },
    CommandDef {
        names: &["inbox"],
        help: "add a message to the agent's inbox",
        takes_arg: true,
        effect: |arg, _, _| match arg {
            Some(body) if !body.is_empty() => CommandResult {
                response: "Item added to inbox.".to_string(),
                side_effect: Some(CommandSideEffect::InboxAdd(body.to_string())),
            },
            _ => CommandResult {
                response: "usage: /inbox <text>".to_string(),
                side_effect: None,
            },
        },
    },
];

/// Execute a command by name with optional arguments.
///
/// Separates the response text from the side effect so that each channel
/// (Discord, Telegram) only needs to handle transport-specific actions.
/// Unknown commands return an error response with no side effect.
#[must_use]
pub fn execute_command(name: &str, args: Option<&str>, ctx: &CommandContext<'_>) -> CommandResult {
    for def in COMMANDS {
        if def.names.contains(&name) {
            return (def.effect)(args, ctx.url, ctx.verbose);
        }
    }

    CommandResult {
        response: format!("unknown command: /{name} (try /help)"),
        side_effect: None,
    }
}

/// Iterate over all commands in the registry.
///
/// Used by interfaces that want to register the full command set.
pub fn all_commands() -> impl Iterator<Item = CommandInfo> {
    COMMANDS.iter().map(|def| CommandInfo {
        name: def
            .names
            .first()
            .copied()
            .unwrap_or_else(|| unreachable!("CommandDef must have at least one name")),
        help: def.help,
        takes_arg: def.takes_arg,
    })
}

/// Build the `/help` response text.
fn help_text() -> String {
    let mut lines = vec!["Available commands:".to_string()];
    for def in COMMANDS {
        let aliases = def.names.join(", /");
        let arg_hint = if def.takes_arg { " <text>" } else { "" };
        lines.push(format!(
            "  /{}{:<14}\u{2014} {}",
            aliases, arg_hint, def.help
        ));
    }
    lines.join("\n")
}

fn status_text(url: &str, verbose: bool) -> String {
    let verbose_label = if verbose { "on" } else { "off" };
    format!("connected to: {url}\nverbose: {verbose_label}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CommandContext<'static> {
        CommandContext {
            url: "ws://localhost/ws",
            verbose: false,
        }
    }

    #[test]
    fn execute_help_returns_text() {
        let result = execute_command("help", None, &ctx());
        assert!(
            result.response.contains("Available"),
            "should return help text: {}",
            result.response
        );
        assert!(result.side_effect.is_none(), "help has no side effect");
    }

    #[test]
    fn execute_help_alias() {
        let result = execute_command("h", None, &ctx());
        assert!(
            result.response.contains("Available"),
            "should return help text via /h alias"
        );
    }

    #[test]
    fn execute_status_returns_url_and_verbose_state() {
        let result = execute_command(
            "status",
            None,
            &CommandContext {
                url: "ws://test/ws",
                verbose: true,
            },
        );
        assert!(result.response.contains("ws://test/ws"));
        assert!(result.response.contains("verbose: on"));
    }

    #[test]
    fn execute_reload_returns_side_effect() {
        let result = execute_command("reload", None, &ctx());
        assert_eq!(result.side_effect, Some(CommandSideEffect::Reload));
    }

    #[test]
    fn execute_observe_returns_server_command() {
        let result = execute_command("observe", None, &ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::ServerCommand {
                name: "observe",
                args: None
            })
        );
    }

    #[test]
    fn execute_reflect_returns_server_command() {
        let result = execute_command("reflect", None, &ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::ServerCommand {
                name: "reflect",
                args: None
            })
        );
    }

    #[test]
    fn execute_context_returns_server_command() {
        let result = execute_command("context", None, &ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::ServerCommand {
                name: "context",
                args: None
            })
        );
    }

    #[test]
    fn execute_stop_returns_stop_side_effect() {
        let result = execute_command("stop", None, &ctx());
        assert_eq!(result.side_effect, Some(CommandSideEffect::Stop));
    }

    #[test]
    fn execute_inbox_with_text_returns_inbox_add() {
        let result = execute_command("inbox", Some("remember this"), &ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::InboxAdd("remember this".to_string()))
        );
    }

    #[test]
    fn execute_inbox_empty_returns_usage() {
        let result = execute_command("inbox", None, &ctx());
        assert!(
            result.response.contains("usage"),
            "should show usage: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn execute_unknown_returns_error() {
        let result = execute_command("foobar", None, &ctx());
        assert!(
            result.response.contains("unknown command"),
            "should report unknown: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn all_commands_includes_everything() {
        let cmds: Vec<_> = all_commands().collect();
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(names.contains(&"help"), "should include help");
        assert!(names.contains(&"status"), "should include status");
        assert!(names.contains(&"observe"), "should include observe");
        assert!(names.contains(&"inbox"), "should include inbox");
    }
}
