import Foundation

/// File attachment metadata received from the daemon.
struct FileAttachmentData {
    let filename: String
    let mimeType: String
    let size: Int
    /// Relative URL path, e.g. "/api/files/{id}".
    let url: String
}

/// A single message in an agent's conversation.
struct ChatMessage: Identifiable {
    let id: UUID
    let role: Role
    /// The text content of the message.
    var content: String
    /// Tool calls associated with this agent turn (assistant messages only).
    var toolCalls: [ToolCallData]
    /// File attachment, if this message carries one.
    var fileAttachment: FileAttachmentData?

    enum Role {
        case user
        case assistant
        case system       // centred italic — simple one-line notices
        case systemBlock  // blue-bordered monospace block — structured output (/help, /status)
    }

    init(id: UUID = UUID(), role: Role, content: String, toolCalls: [ToolCallData] = [], fileAttachment: FileAttachmentData? = nil) {
        self.id = id
        self.role = role
        self.content = content
        self.toolCalls = toolCalls
        self.fileAttachment = fileAttachment
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
