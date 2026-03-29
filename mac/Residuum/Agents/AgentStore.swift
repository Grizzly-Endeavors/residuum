import Foundation
import Observation

/// Central store for all agent tabs and their conversations.
///
/// Inject into the SwiftUI environment via `.environment(agentStore)` and
/// read with `@Environment(AgentStore.self)` in views.
@Observable
final class AgentStore {
    // MARK: - Public state (observed by SwiftUI)

    var tabs: [AgentTab] = []
    var selectedTabId: UUID?
    var host: String

    /// The currently selected agent tab, or the first tab if none is selected.
    var selectedTab: AgentTab? {
        guard let id = selectedTabId else { return tabs.first }
        return tabs.first { $0.id == id }
    }

    /// Index of the currently selected tab.
    var selectedTabIndex: Int? {
        guard let id = selectedTabId else { return tabs.isEmpty ? nil : 0 }
        return tabs.firstIndex { $0.id == id }
    }

    /// True if the default agent (first tab) is connected.
    var defaultAgentConnected: Bool {
        tabs.first?.connection.state == .connected
    }

    // MARK: - Init

    init(host: String = "127.0.0.1") {
        self.host = host
        loadAgents()
    }

    // MARK: - Public API

    /// Select the given agent tab.
    func select(_ tab: AgentTab) {
        selectedTabId = tab.id
    }

    /// Send a message on the currently selected agent tab.
    func sendMessage(content: String, images: [ImageData] = []) {
        guard let idx = selectedTabIndex else { return }
        let correlationId = UUID().uuidString
        let userMsg = ChatMessage(role: .user, content: content)
        tabs[idx].messages.append(userMsg)
        tabs[idx].pendingCorrelationId = correlationId
        tabs[idx].connection.send(.sendMessage(id: correlationId, content: content, images: images))
    }

    /// Force-reconnect a specific tab's connection.
    func reconnect(tab: AgentTab) {
        guard let idx = tabs.firstIndex(where: { $0.id == tab.id }) else { return }
        tabs[idx].connection.disconnect()
        tabs[idx].connection.connect()
    }

    /// Appends a centred italic system notice to the selected tab's feed.
    func appendSystemMessage(_ content: String) {
        guard let idx = selectedTabIndex else { return }
        tabs[idx].messages.append(ChatMessage(role: .system, content: content))
    }

    /// Appends a blue-bordered monospace block to the selected tab's feed.
    func appendSystemBlock(_ content: String) {
        guard let idx = selectedTabIndex else { return }
        tabs[idx].messages.append(ChatMessage(role: .systemBlock, content: content))
    }

    /// Send a ClientMessage to the currently selected tab's connection.
    func sendToSelectedTab(_ message: ClientMessage) {
        guard let idx = selectedTabIndex else { return }
        tabs[idx].connection.send(message)
    }

    /// Update the host and reconnect all agent connections.
    func reconnectAll(host newHost: String) {
        host = newHost
        for idx in tabs.indices {
            tabs[idx].connection.disconnect()
            let newConn = ResiduumConnection(host: newHost, port: tabs[idx].port)
            tabs[idx].connection = newConn
            wireHandlers(tabIndex: idx)
            tabs[idx].connection.connect()
        }
    }

    // MARK: - Setup

    private func loadAgents() {
        // Default agent is always first (port 7700).
        let defaultConn = makeConnection(port: 7700)
        let defaultTab = AgentTab(name: "Default", port: 7700, connection: defaultConn)
        tabs.append(defaultTab)

        // Named agents from registry.
        let registry = AgentRegistry.load()
        for entry in registry.agents {
            let conn = makeConnection(port: entry.port)
            let tab = AgentTab(name: entry.name, port: entry.port, connection: conn)
            tabs.append(tab)
        }

        selectedTabId = tabs.first?.id

        // Wire up handlers and connect all.
        for idx in tabs.indices {
            wireHandlers(tabIndex: idx)
            tabs[idx].connection.connect()
        }
    }

    private func makeConnection(port: UInt16) -> ResiduumConnection {
        ResiduumConnection(host: host, port: port)
    }

    private func wireHandlers(tabIndex: Int) {
        let tabId = tabs[tabIndex].id
        tabs[tabIndex].connection.onMessage = { [weak self] message in
            guard let self, let idx = self.tabs.firstIndex(where: { $0.id == tabId }) else { return }
            self.handle(message, tabIndex: idx)
        }
        tabs[tabIndex].connection.onStateChange = { [weak self] _ in
            // Accessing tabs triggers @Observable to notify views of the state change.
            guard let self, let idx = self.tabs.firstIndex(where: { $0.id == tabId }) else { return }
            _ = self.tabs[idx].connection.state
        }
    }

    // MARK: - Message handling

    private func handle(_ message: ServerMessage, tabIndex: Int) {
        switch message {
        case .turnStarted(let correlationId):
            tabs[tabIndex].isThinking = true
            tabs[tabIndex].pendingCorrelationId = correlationId
            tabs[tabIndex].pendingToolCalls = []

        case .toolCall(let id, let name, let arguments):
            let call = ToolCallData(id: id, name: name, arguments: arguments, isError: false)
            tabs[tabIndex].pendingToolCalls.append(call)

        case .toolResult(let toolCallId, _, let output, let isError):
            if let idx = tabs[tabIndex].pendingToolCalls.firstIndex(where: { $0.id == toolCallId }) {
                tabs[tabIndex].pendingToolCalls[idx].result = output
                tabs[tabIndex].pendingToolCalls[idx].isError = isError
            }

        case .response(_, let content):
            var assistantMsg = ChatMessage(role: .assistant, content: content)
            assistantMsg.toolCalls = tabs[tabIndex].pendingToolCalls
            tabs[tabIndex].messages.append(assistantMsg)
            tabs[tabIndex].isThinking = false
            tabs[tabIndex].pendingToolCalls = []
            tabs[tabIndex].pendingCorrelationId = nil

        case .broadcastResponse(let content):
            // Intermediate text emitted alongside tool calls.
            // Skip while thinking — the final response will replace it.
            if tabs[tabIndex].isThinking { break }
            let msg = ChatMessage(role: .assistant, content: content)
            tabs[tabIndex].messages.append(msg)

        case .systemEvent(let source, let content):
            let msg = ChatMessage(role: .system, content: "[\(source)] \(content)")
            tabs[tabIndex].messages.append(msg)

        case .notice(let message):
            let msg = ChatMessage(role: .system, content: message)
            tabs[tabIndex].messages.append(msg)

        case .error(_, let message):
            tabs[tabIndex].isThinking = false
            let msg = ChatMessage(role: .system, content: "Error: \(message)")
            tabs[tabIndex].messages.append(msg)

        case .reloading:
            let msg = ChatMessage(role: .system, content: "Reloading configuration…")
            tabs[tabIndex].messages.append(msg)

        case .pong, .unknown:
            break
        }
    }
}
