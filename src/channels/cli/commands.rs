//! Data-driven slash command registry for the CLI client.

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
}
