import Foundation

/// A single message in an agent's conversation.
struct ChatMessage: Identifiable {
    let id: UUID
    let role: Role
    /// The text content of the message.
    var content: String
    /// Tool calls associated with this agent turn (assistant messages only).
    var toolCalls: [ToolCallData]

    enum Role {
        case user
        case assistant
        case system   // notices and system events
    }

    init(id: UUID = UUID(), role: Role, content: String, toolCalls: [ToolCallData] = []) {
        self.id = id
        self.role = role
        self.content = content
        self.toolCalls = toolCalls
    }
}

/// A tool invocation and its result within an assistant turn.
struct ToolCallData: Identifiable {
    /// The tool_call_id from the daemon.
    let id: String
    let name: String
    let arguments: [String: JSONValue]
    /// Populated when the tool_result arrives.
    var result: String?
    var isError: Bool
}
