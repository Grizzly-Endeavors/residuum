//! Data-driven slash command registry for the CLI client.
//!
//! Provides a shared command registry used by CLI, Discord, and any future channels.
//! Each channel handles `CommandSideEffect` according to its own transport.

/// What the main loop should do after a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEffect {
    /// Print text locally, no server message.
    PrintLocal(String),
    /// Toggle verbose mode (local + server `SetVerbose` message).
    ToggleVerbose,
    /// Send a named server command.
    ServerCommand {
        /// Command name dispatched to the gateway event loop.
        name: &'static str,
        /// Optional argument text.
        args: Option<String>,
    },
    /// Send a reload request (special lifecycle).
    Reload,
    /// Send an inbox add (async file I/O in ws.rs).
    InboxAdd(String),
    /// Exit the client.
    Quit,
}

/// Metadata about a server command, for cross-channel registration.
pub struct ServerCommandInfo {
    /// The primary command name (e.g. "observe").
    pub name: &'static str,
    /// Human-readable help text.
    pub help: &'static str,
}

/// Metadata about any command, for cross-channel registration.
pub struct CommandInfo {
    /// The primary command name (e.g. "help", "observe").
    pub name: &'static str,
    /// Human-readable help text.
    pub help: &'static str,
    /// Whether the command takes a text argument.
    pub takes_arg: bool,
}

/// Context for executing a command from any channel.
pub struct CommandContext<'a> {
    /// Connection URL (for status display).
    pub url: &'a str,
    /// Whether verbose mode is enabled.
    pub verbose: bool,
    /// Name of the channel dispatching the command (e.g. "cli", "discord", "websocket").
    pub channel_name: &'a str,
}

/// Result of executing a command through the shared registry.
pub struct CommandResult {
    /// Text response to display to the user.
    pub response: String,
    /// Optional side effect the channel handler must apply.
    pub side_effect: Option<CommandSideEffect>,
}

/// Side effects that channel handlers must apply after a command.
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
    /// Exit the client (CLI-only; other channels ignore this).
    Quit,
    /// Toggle verbose mode (CLI-only; other channels ignore this).
    ToggleVerbose,
}

struct CommandDef {
    names: &'static [&'static str],
    help: &'static str,
    takes_arg: bool,
    effect: fn(arg: Option<&str>, url: &str, verbose: bool) -> CommandEffect,
}

static COMMANDS: &[CommandDef] = &[
    CommandDef {
        names: &["help", "h"],
        help: "show this help",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::PrintLocal(help_text()),
    },
    CommandDef {
        names: &["status"],
        help: "show connection info",
        takes_arg: false,
        effect: |_, url, verbose| CommandEffect::PrintLocal(status_text(url, verbose)),
    },
    CommandDef {
        names: &["verbose", "v"],
        help: "toggle verbose mode (tool events)",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::ToggleVerbose,
    },
    CommandDef {
        names: &["reload", "r"],
        help: "reload server configuration",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::Reload,
    },
    CommandDef {
        names: &["observe", "obs"],
        help: "force a memory observation cycle",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::ServerCommand {
            name: "observe",
            args: None,
        },
    },
    CommandDef {
        names: &["reflect", "ref"],
        help: "force a reflection cycle",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::ServerCommand {
            name: "reflect",
            args: None,
        },
    },
    CommandDef {
        names: &["context", "ctx"],
        help: "show context token usage",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::ServerCommand {
            name: "context",
            args: None,
        },
    },
    CommandDef {
        names: &["inbox"],
        help: "add a message to the agent's inbox",
        takes_arg: true,
        effect: |arg, _, _| match arg {
            Some(body) if !body.is_empty() => CommandEffect::InboxAdd(body.to_string()),
            _ => CommandEffect::PrintLocal("usage: /inbox <text>".to_string()),
        },
    },
    CommandDef {
        names: &["quit", "exit", "q"],
        help: "disconnect and exit",
        takes_arg: false,
        effect: |_, _, _| CommandEffect::Quit,
    },
];

/// Parse a line of user input as a slash command.
///
/// Returns `None` if the input does not start with `/`.
#[must_use]
pub fn parse_command(input: &str, url: &str, verbose: bool) -> Option<CommandEffect> {
    let trimmed = input.trim();
    let cmd_str = trimmed.strip_prefix('/')?;
    let keyword = cmd_str.split_whitespace().next().unwrap_or(cmd_str);

    for def in COMMANDS {
        if def.names.contains(&keyword) {
            let arg = if def.takes_arg {
                cmd_str
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim())
            } else {
                None
            };
            return Some((def.effect)(arg, url, verbose));
        }
    }

    Some(CommandEffect::PrintLocal(format!(
        "unknown command: /{keyword} (try /help)"
    )))
}

/// Execute a command by name with optional arguments.
///
/// Separates the response text from the side effect so that each channel
/// (CLI, Discord, WebSocket) only needs to handle transport-specific
/// actions. Unknown commands return an error response with no side effect.
#[must_use]
pub fn execute_command(name: &str, args: Option<&str>, ctx: &CommandContext<'_>) -> CommandResult {
    for def in COMMANDS {
        if def.names.contains(&name) {
            let effect = (def.effect)(args, ctx.url, ctx.verbose);
            return effect_to_result(effect);
        }
    }

    CommandResult {
        response: format!("unknown command: /{name} (try /help)"),
        side_effect: None,
    }
}

/// Convert a `CommandEffect` into a `CommandResult`.
fn effect_to_result(effect: CommandEffect) -> CommandResult {
    match effect {
        CommandEffect::PrintLocal(text) => CommandResult {
            response: text,
            side_effect: None,
        },
        CommandEffect::ToggleVerbose => CommandResult {
            response: "verbose mode toggled".to_string(),
            side_effect: Some(CommandSideEffect::ToggleVerbose),
        },
        CommandEffect::ServerCommand { name, args } => CommandResult {
            response: format!("{name} triggered."),
            side_effect: Some(CommandSideEffect::ServerCommand { name, args }),
        },
        CommandEffect::Reload => CommandResult {
            response: "Reloading configuration...".to_string(),
            side_effect: Some(CommandSideEffect::Reload),
        },
        CommandEffect::InboxAdd(body) => CommandResult {
            response: "Item added to inbox.".to_string(),
            side_effect: Some(CommandSideEffect::InboxAdd(body)),
        },
        CommandEffect::Quit => CommandResult {
            response: "Disconnecting...".to_string(),
            side_effect: Some(CommandSideEffect::Quit),
        },
    }
}

/// Iterate over commands that produce `ServerCommand` effects.
///
/// Used by Discord (and potentially other channels) to auto-register
/// server commands without duplicating the list.
pub fn server_commands() -> impl Iterator<Item = ServerCommandInfo> {
    COMMANDS.iter().filter_map(|def| {
        let effect = (def.effect)(None, "", false);
        matches!(effect, CommandEffect::ServerCommand { .. }).then(|| ServerCommandInfo {
            name: def.names.first().copied().unwrap_or(""),
            help: def.help,
        })
    })
}

/// Iterate over all commands in the registry.
///
/// Used by channels that want to register the full command set
/// (not just server commands).
pub fn all_commands() -> impl Iterator<Item = CommandInfo> {
    COMMANDS.iter().map(|def| CommandInfo {
        name: def.names.first().copied().unwrap_or(""),
        help: def.help,
        takes_arg: def.takes_arg,
    })
}

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

    #[test]
    fn parse_help() {
        let effect = parse_command("/help", "ws://localhost/ws", false);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("Available")),
            "should produce help text"
        );
    }

    #[test]
    fn parse_help_alias() {
        let effect = parse_command("/h", "ws://localhost/ws", false);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("Available")),
            "should produce help text via /h alias"
        );
    }

    #[test]
    fn parse_status() {
        let effect = parse_command("/status", "ws://test/ws", true);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("ws://test/ws") && text.contains("verbose: on")),
            "should produce status text with url and verbose state"
        );
    }

    #[test]
    fn parse_verbose() {
        assert_eq!(
            parse_command("/verbose", "", false),
            Some(CommandEffect::ToggleVerbose),
            "should parse /verbose"
        );
        assert_eq!(
            parse_command("/v", "", false),
            Some(CommandEffect::ToggleVerbose),
            "should parse /v alias"
        );
    }

    #[test]
    fn parse_reload() {
        assert_eq!(
            parse_command("/reload", "", false),
            Some(CommandEffect::Reload),
            "should parse /reload"
        );
        assert_eq!(
            parse_command("/r", "", false),
            Some(CommandEffect::Reload),
            "should parse /r alias"
        );
    }

    #[test]
    fn parse_observe() {
        assert_eq!(
            parse_command("/observe", "", false),
            Some(CommandEffect::ServerCommand {
                name: "observe",
                args: None
            }),
            "should parse /observe"
        );
        assert_eq!(
            parse_command("/obs", "", false),
            Some(CommandEffect::ServerCommand {
                name: "observe",
                args: None
            }),
            "should parse /obs alias"
        );
    }

    #[test]
    fn parse_reflect() {
        assert_eq!(
            parse_command("/reflect", "", false),
            Some(CommandEffect::ServerCommand {
                name: "reflect",
                args: None
            }),
            "should parse /reflect"
        );
        assert_eq!(
            parse_command("/ref", "", false),
            Some(CommandEffect::ServerCommand {
                name: "reflect",
                args: None
            }),
            "should parse /ref alias"
        );
    }

    #[test]
    fn parse_context() {
        assert_eq!(
            parse_command("/context", "", false),
            Some(CommandEffect::ServerCommand {
                name: "context",
                args: None
            }),
            "should parse /context"
        );
        assert_eq!(
            parse_command("/ctx", "", false),
            Some(CommandEffect::ServerCommand {
                name: "context",
                args: None
            }),
            "should parse /ctx alias"
        );
    }

    #[test]
    fn parse_inbox_with_text() {
        assert_eq!(
            parse_command("/inbox hello world", "", false),
            Some(CommandEffect::InboxAdd("hello world".to_string())),
            "should parse /inbox with body"
        );
    }

    #[test]
    fn parse_inbox_empty_returns_usage() {
        let effect = parse_command("/inbox", "", false);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("usage")),
            "/inbox with no text should return usage message"
        );
    }

    #[test]
    fn parse_quit_variants() {
        assert_eq!(
            parse_command("/quit", "", false),
            Some(CommandEffect::Quit),
            "should parse /quit"
        );
        assert_eq!(
            parse_command("/exit", "", false),
            Some(CommandEffect::Quit),
            "should parse /exit"
        );
        assert_eq!(
            parse_command("/q", "", false),
            Some(CommandEffect::Quit),
            "should parse /q"
        );
    }

    #[test]
    fn parse_unknown() {
        let effect = parse_command("/foobar", "", false);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("unknown command")),
            "should return unknown command message"
        );
    }

    #[test]
    fn parse_non_slash_returns_none() {
        assert_eq!(
            parse_command("hello world", "", false),
            None,
            "non-slash input should return None"
        );
    }

    #[test]
    fn parse_with_leading_whitespace() {
        let effect = parse_command("  /help", "", false);
        assert!(
            matches!(&effect, Some(CommandEffect::PrintLocal(text)) if text.contains("Available")),
            "should handle leading whitespace"
        );
    }

    #[test]
    fn help_text_contains_all_commands() {
        let text = help_text();
        for def in COMMANDS {
            for name in def.names {
                assert!(text.contains(name), "help text should mention /{name}");
            }
        }
    }

    #[test]
    fn server_commands_returns_expected() {
        let cmds: Vec<_> = server_commands().collect();
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(names.contains(&"observe"), "should include observe");
        assert!(names.contains(&"reflect"), "should include reflect");
        assert!(names.contains(&"context"), "should include context");
        assert!(
            !names.contains(&"help"),
            "help is local, not a server command"
        );
        assert!(
            !names.contains(&"quit"),
            "quit is local, not a server command"
        );
    }

    // ── execute_command tests ─────────────────────────────────────────

    fn cli_ctx() -> CommandContext<'static> {
        CommandContext {
            url: "ws://localhost/ws",
            verbose: false,
            channel_name: "cli",
        }
    }

    #[test]
    fn execute_help_returns_text() {
        let result = execute_command("help", None, &cli_ctx());
        assert!(
            result.response.contains("Available"),
            "should return help text: {}",
            result.response
        );
        assert!(result.side_effect.is_none(), "help has no side effect");
    }

    #[test]
    fn execute_observe_returns_server_command() {
        let result = execute_command("observe", None, &cli_ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::ServerCommand {
                name: "observe",
                args: None
            })
        );
    }

    #[test]
    fn execute_inbox_with_text_returns_inbox_add() {
        let result = execute_command("inbox", Some("remember this"), &cli_ctx());
        assert_eq!(
            result.side_effect,
            Some(CommandSideEffect::InboxAdd("remember this".to_string()))
        );
    }

    #[test]
    fn execute_inbox_empty_returns_usage() {
        let result = execute_command("inbox", None, &cli_ctx());
        assert!(
            result.response.contains("usage"),
            "should show usage: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn execute_unknown_returns_error() {
        let result = execute_command("foobar", None, &cli_ctx());
        assert!(
            result.response.contains("unknown command"),
            "should report unknown: {}",
            result.response
        );
        assert!(result.side_effect.is_none());
    }

    #[test]
    fn execute_reload_returns_side_effect() {
        let result = execute_command("reload", None, &cli_ctx());
        assert_eq!(result.side_effect, Some(CommandSideEffect::Reload));
    }

    #[test]
    fn all_commands_includes_everything() {
        let cmds: Vec<_> = all_commands().collect();
        let names: Vec<_> = cmds.iter().map(|c| c.name).collect();
        assert!(names.contains(&"help"), "should include help");
        assert!(names.contains(&"status"), "should include status");
        assert!(names.contains(&"observe"), "should include observe");
        assert!(names.contains(&"inbox"), "should include inbox");
        assert!(names.contains(&"quit"), "should include quit");
    }
}
