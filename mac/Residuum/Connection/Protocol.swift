import Foundation

// MARK: - ImageData

/// Base64-encoded image attachment. Matches the Rust `ImageData` struct exactly.
struct ImageData: Codable {
    /// MIME type, e.g. `"image/png"`, `"image/jpeg"`.
    let mediaType: String
    /// Base64-encoded image bytes.
    let data: String

    enum CodingKeys: String, CodingKey {
        case mediaType = "media_type"
        case data
    }
}

// MARK: - ClientMessage

/// Messages sent from this app to the Residuum daemon.
/// Matches the Rust `ClientMessage` enum with `snake_case` JSON tags.
enum ClientMessage: Encodable {
    case sendMessage(id: String, content: String, images: [ImageData])
    case setVerbose(enabled: Bool)
    case ping
    case reload
    case serverCommand(name: String, args: String?)
    case inboxAdd(body: String)

    private enum CodingKeys: String, CodingKey {
        case type, id, content, images, enabled, name, args, body
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .sendMessage(let id, let content, let images):
            try c.encode("send_message", forKey: .type)
            try c.encode(id, forKey: .id)
            try c.encode(content, forKey: .content)
            if !images.isEmpty {
                try c.encode(images, forKey: .images)
            }
        case .setVerbose(let enabled):
            try c.encode("set_verbose", forKey: .type)
            try c.encode(enabled, forKey: .enabled)
        case .ping:
            try c.encode("ping", forKey: .type)
        case .reload:
            try c.encode("reload", forKey: .type)
        case .serverCommand(let name, let args):
            try c.encode("server_command", forKey: .type)
            try c.encode(name, forKey: .name)
            try c.encodeIfPresent(args, forKey: .args)
        case .inboxAdd(let body):
            try c.encode("inbox_add", forKey: .type)
            try c.encode(body, forKey: .body)
        }
    }
}

// MARK: - ServerMessage

/// Messages received from the Residuum daemon.
/// Matches the Rust `ServerMessage` enum with `snake_case` JSON tags.
enum ServerMessage: Decodable {
    case turnStarted(replyTo: String)
    case toolCall(id: String, name: String, arguments: [String: JSONValue])
    case toolResult(toolCallId: String, name: String, output: String, isError: Bool)
    case response(replyTo: String, content: String)
    case systemEvent(source: String, content: String)
    case broadcastResponse(content: String)
    case error(replyTo: String?, message: String)
    case notice(message: String)
    case pong
    case reloading
    case unknown

    private enum CodingKeys: String, CodingKey {
        case type
        case replyTo = "reply_to"
        case id, name, arguments
        case toolCallId = "tool_call_id"
        case output
        case isError = "is_error"
        case source, content, message
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "turn_started":
            self = .turnStarted(replyTo: try c.decode(String.self, forKey: .replyTo))
        case "tool_call":
            self = .toolCall(
                id: try c.decode(String.self, forKey: .id),
                name: try c.decode(String.self, forKey: .name),
                arguments: try c.decode([String: JSONValue].self, forKey: .arguments)
            )
        case "tool_result":
            self = .toolResult(
                toolCallId: try c.decode(String.self, forKey: .toolCallId),
                name: try c.decode(String.self, forKey: .name),
                output: try c.decode(String.self, forKey: .output),
                isError: try c.decode(Bool.self, forKey: .isError)
            )
        case "response":
            self = .response(
                replyTo: try c.decode(String.self, forKey: .replyTo),
                content: try c.decode(String.self, forKey: .content)
            )
        case "system_event":
            self = .systemEvent(
                source: try c.decode(String.self, forKey: .source),
                content: try c.decode(String.self, forKey: .content)
            )
        case "broadcast_response":
            self = .broadcastResponse(content: try c.decode(String.self, forKey: .content))
        case "error":
            self = .error(
                replyTo: try? c.decode(String.self, forKey: .replyTo),
                message: try c.decode(String.self, forKey: .message)
            )
        case "notice":
            self = .notice(message: try c.decode(String.self, forKey: .message))
        case "pong":
            self = .pong
        case "reloading":
            self = .reloading
        default:
            self = .unknown
        }
    }
}

// MARK: - JSONValue

/// A type-erased JSON value for decoding tool call arguments,
/// which can be any valid JSON structure.
enum JSONValue: Decodable, CustomStringConvertible, Equatable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case null
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let v = try? c.decode(Bool.self)    { self = .bool(v); return }
        if let v = try? c.decode(Double.self)  { self = .number(v); return }
        if let v = try? c.decode(String.self)  { self = .string(v); return }
        if let v = try? c.decode([JSONValue].self) { self = .array(v); return }
        if let v = try? c.decode([String: JSONValue].self) { self = .object(v); return }
        self = .null
    }

    var description: String {
        switch self {
        case .string(let s): return s
        case .number(let n): return n.truncatingRemainder(dividingBy: 1) == 0
            ? String(Int(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        case .array(let a): return "[\(a.map(\.description).joined(separator: ", "))]"
        case .object(let o): return "{\(o.map { "\($0.key): \($0.value)" }.joined(separator: ", "))}"
        }
    }
}
