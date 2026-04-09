import Foundation

/// A single slash command available in the input bar.
struct SlashCommand: Identifiable {
    var id: String { name }
    /// The command name including the leading slash, e.g. `"/observe"`.
    let name: String
    /// Short description shown in the autocomplete menu.
    let description: String
    /// True only for commands that take an argument (currently only `/inbox`).
    let hasArgs: Bool
}

/// All available slash commands, in display order.
let COMMAND_REGISTRY: [SlashCommand] = [
    SlashCommand(name: "/help",    description: "Show this help message",       hasArgs: false),
    SlashCommand(name: "/verbose", description: "Toggle tool call visibility",  hasArgs: false),
    SlashCommand(name: "/status",  description: "Show connection status",       hasArgs: false),
    SlashCommand(name: "/observe", description: "Trigger memory observation",   hasArgs: false),
    SlashCommand(name: "/reflect", description: "Trigger memory reflection",    hasArgs: false),
    SlashCommand(name: "/context", description: "Show current project context", hasArgs: false),
    SlashCommand(name: "/reload",  description: "Reload gateway configuration", hasArgs: false),
    SlashCommand(name: "/inbox",   description: "Add a message to the inbox",   hasArgs: true),
]
