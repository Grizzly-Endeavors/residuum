import Foundation

/// One tab in the UI, corresponding to one Residuum agent daemon.
struct AgentTab: Identifiable {
    let id: UUID
    /// Display name — "Default" for the default agent, agent name for named agents.
    let name: String
    let port: UInt16
    /// The live WebSocket connection to this agent's daemon.
    var connection: ResiduumConnection
    /// All messages in this agent's conversation (chronological order).
    var messages: [ChatMessage]
    /// True while the agent is processing a turn (between turn_started and response).
    var isThinking: Bool
    /// The pending tool group being accumulated during the current turn.
    var pendingToolCalls: [ToolCallData]
    /// Correlation ID of the in-flight turn, for matching response/error to turn.
    var pendingCorrelationId: String?
    /// Whether tool calls and results are shown for this agent's feed.
    /// Toggled by the /verbose command.
    var verboseEnabled: Bool = false

    init(name: String, port: UInt16, connection: ResiduumConnection) {
        self.id = UUID()
        self.name = name
        self.port = port
        self.connection = connection
        self.messages = []
        self.isThinking = false
        self.pendingToolCalls = []
    }
}
