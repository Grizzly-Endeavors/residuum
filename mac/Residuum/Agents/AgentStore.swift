import Foundation
import Observation

/// The subset of the hub's `AgentSummary` this app needs to pick an agent.
private struct HubAgentSummary: Decodable {
    let name: String
    let state: String
}

private struct HubAgentList: Decodable {
    let agents: [HubAgentSummary]
}

/// Store for the single conversation with one agent on the Residuum hub.
///
/// Inject into the SwiftUI environment via `.environment(agentStore)` and
/// read with `@Environment(AgentStore.self)` in views.
@Observable
final class AgentStore {
    // MARK: - Public state (observed by SwiftUI)

    var host: String
    /// Agent name from Settings. Empty means "first running agent".
    var agent: String
    /// The agent this conversation is connected (or connecting) to.
    var agentName: String?
    var connectionState: ConnectionState = .disconnected
    /// Plain-language reason the app cannot chat yet; nil otherwise.
    var problem: String?
    /// All messages in the conversation (chronological order).
    var messages: [ChatMessage] = []
    /// True while the agent is processing a turn (between turn_started and response).
    var isThinking = false
    /// Whether tool calls and results are shown. Toggled by the /verbose command.
    var verboseEnabled = false

    // MARK: - Private

    let port: UInt16 = 7700
    private var connection: ResiduumConnection?
    private var connectTask: Task<Void, Never>?
    /// The pending tool group being accumulated during the current turn.
    private var pendingToolCalls: [ToolCallData] = []
    /// Correlation ID of the in-flight turn, for matching response/error to turn.
    private var pendingCorrelationId: String?

    // MARK: - Init

    init() {
        host = UserDefaults.standard.string(forKey: "residuum.host") ?? "127.0.0.1"
        agent = UserDefaults.standard.string(forKey: "residuum.agent") ?? ""
        connect()
    }

    // MARK: - Public API

    /// Send a message to the agent.
    func sendMessage(content: String, images: [ImageData] = []) {
        let correlationId = UUID().uuidString
        messages.append(ChatMessage(role: .user, content: content))
        pendingCorrelationId = correlationId
        connection?.send(.sendMessage(id: correlationId, content: content, images: images))
    }

    /// Send a ClientMessage to the agent.
    func send(_ message: ClientMessage) {
        connection?.send(message)
    }

    /// Toggle tool call visibility and tell the agent.
    func toggleVerbose() {
        verboseEnabled.toggle()
        connection?.send(.setVerbose(enabled: verboseEnabled))
    }

    /// Appends a centred italic system notice to the feed.
    func appendSystemMessage(_ content: String) {
        messages.append(ChatMessage(role: .system, content: content))
    }

    /// Appends a blue-bordered monospace block to the feed.
    func appendSystemBlock(_ content: String) {
        messages.append(ChatMessage(role: .systemBlock, content: content))
    }

    /// Apply new settings and reconnect. Clears the feed when the target changed.
    func reconnect(host newHost: String, agent newAgent: String) {
        if newHost != host || newAgent != agent {
            messages = []
        }
        host = newHost
        agent = newAgent
        connect()
    }

    /// Drop the current connection and connect again, re-resolving the agent.
    func connect() {
        connectTask?.cancel()
        connection?.disconnect()
        connection = nil
        problem = nil
        isThinking = false
        pendingToolCalls = []
        pendingCorrelationId = nil
        connectionState = .connecting
        connectTask = Task { @MainActor [weak self] in
            await self?.resolveAndConnect()
        }
    }

    // MARK: - Setup

    @MainActor
    private func resolveAndConnect() async {
        let configured = agent.trimmingCharacters(in: .whitespaces)
        let name: String
        if configured.isEmpty {
            do {
                guard let first = try await firstRunningAgent() else {
                    fail("No agent is running. Start one from the web interface or with `residuum agent start`, then reconnect.")
                    return
                }
                name = first
            } catch {
                fail("Couldn't reach the Residuum hub at \(host):\(port). Make sure it is running and check the host in Settings.")
                return
            }
        } else {
            name = configured
        }
        if Task.isCancelled { return }

        agentName = name
        let conn = ResiduumConnection(host: host, port: port, agent: name)
        conn.onMessage = { [weak self] message in
            self?.handle(message)
        }
        conn.onStateChange = { [weak self] state in
            guard let self else { return }
            self.connectionState = state
            if state == .connected {
                // The connection resets the server-side verbose flag on every open.
                self.verboseEnabled = false
            }
        }
        connection = conn
        conn.connect()
    }

    @MainActor
    private func fail(_ message: String) {
        problem = message
        connectionState = .disconnected
    }

    /// Asks the hub for its agents and returns the first running one (the API sorts by name).
    private func firstRunningAgent() async throws -> String? {
        var components = URLComponents()
        components.scheme = "http"
        components.host = host
        components.port = Int(port)
        components.path = "/api/hub/agents"
        guard let url = components.url else { throw URLError(.badURL) }
        var request = URLRequest(url: url)
        request.timeoutInterval = 5
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw URLError(.badServerResponse)
        }
        let list = try JSONDecoder().decode(HubAgentList.self, from: data)
        return list.agents.first { $0.state == "running" }?.name
    }

    // MARK: - Message handling

    private func handle(_ message: ServerMessage) {
        switch message {
        case .turnStarted(let correlationId):
            isThinking = true
            pendingCorrelationId = correlationId
            pendingToolCalls = []

        case .toolCall(let id, let name, let arguments):
            let call = ToolCallData(id: id, name: name, arguments: arguments, isError: false)
            pendingToolCalls.append(call)

        case .toolResult(let toolCallId, _, let output, let isError):
            if let idx = pendingToolCalls.firstIndex(where: { $0.id == toolCallId }) {
                pendingToolCalls[idx].result = output
                pendingToolCalls[idx].isError = isError
            }

        case .response(_, let content):
            var assistantMsg = ChatMessage(role: .assistant, content: content)
            assistantMsg.toolCalls = pendingToolCalls
            messages.append(assistantMsg)
            isThinking = false
            pendingToolCalls = []
            pendingCorrelationId = nil

        case .broadcastResponse(let content):
            // Intermediate text emitted alongside tool calls.
            // Skip while thinking — the final response will replace it.
            if isThinking { break }
            messages.append(ChatMessage(role: .assistant, content: content))

        case .systemEvent(let source, let content):
            messages.append(ChatMessage(role: .system, content: "[\(source)] \(content)"))

        case .notice(let message):
            messages.append(ChatMessage(role: .system, content: message))

        case .error(_, let message):
            isThinking = false
            messages.append(ChatMessage(role: .system, content: "Error: \(message)"))

        case .fileAttachment(_, let filename, let mimeType, let size, let urlPath, let caption):
            // Pre-compute the absolute URL so views don't need host/port plumbed through them.
            let fullURL = "http://\(host):\(port)\(urlPath)"
            let attachment = FileAttachmentData(filename: filename, mimeType: mimeType, size: size, url: fullURL)
            messages.append(ChatMessage(role: .assistant, content: caption ?? "", fileAttachment: attachment))
            isThinking = false

        case .reloading:
            messages.append(ChatMessage(role: .system, content: "Reloading configuration…"))

        case .pong, .unknown:
            break
        }
    }
}
