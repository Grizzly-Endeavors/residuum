import Foundation

enum ConnectionState: Equatable {
    case connecting
    case connected
    case disconnected
}

/// Manages a single WebSocket connection to one Residuum agent daemon.
///
/// Call `connect()` to start. The connection retries automatically on
/// failure with exponential backoff (1 s → 2 s → 4 s → … → 30 s max).
/// Call `disconnect()` to stop permanently.
///
/// Received messages are delivered on the main queue via `onMessage`.
final class ResiduumConnection: NSObject {
    // MARK: - Public state

    private(set) var state: ConnectionState = .disconnected

    /// Called on the main queue whenever a message arrives from the daemon.
    var onMessage: ((ServerMessage) -> Void)?

    /// Called on the main queue whenever the connection state changes.
    var onStateChange: ((ConnectionState) -> Void)?

    // MARK: - Private

    private let host: String
    private let port: UInt16
    private var task: URLSessionWebSocketTask?
    private lazy var session: URLSession = URLSession(
        configuration: .default,
        delegate: self,
        delegateQueue: nil
    )
    private var pingTimer: Timer?
    private var retryDelay: TimeInterval = 1.0
    private var active = false  // false → don't reconnect after close

    // MARK: - Init

    init(host: String, port: UInt16) {
        self.host = host
        self.port = port
    }

    // MARK: - Public API

    func connect() {
        active = true
        openConnection()
    }

    func disconnect() {
        active = false
        pingTimer?.invalidate()
        pingTimer = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        updateState(.disconnected)
    }

    func send(_ message: ClientMessage) {
        guard state == .connected, let task else { return }
        guard let data = try? JSONEncoder().encode(message),
              let text = String(data: data, encoding: .utf8) else { return }
        // Daemon only accepts Text frames (same as browser WebSocket clients).
        task.send(.string(text)) { [weak self] error in
            if error != nil {
                self?.scheduleReconnect()
            }
        }
    }

    // MARK: - Private

    private func openConnection() {
        var components = URLComponents()
        components.scheme = "ws"
        components.host = host
        components.port = Int(port)
        components.path = "/ws"
        guard let url = components.url else { return }

        updateState(.connecting)
        let t = session.webSocketTask(with: url)
        self.task = t
        t.resume()
        receiveNext()
    }

    private func receiveNext() {
        task?.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let wsMessage):
                let data: Data?
                switch wsMessage {
                case .data(let d):   data = d
                case .string(let s): data = s.data(using: .utf8)
                @unknown default:    data = nil
                }
                if let data, let msg = try? JSONDecoder().decode(ServerMessage.self, from: data) {
                    DispatchQueue.main.async { self.onMessage?(msg) }
                }
                self.receiveNext()
            case .failure:
                self.scheduleReconnect()
            }
        }
    }

    private func scheduleReconnect() {
        DispatchQueue.main.async {
            self.updateState(.disconnected)
            self.pingTimer?.invalidate()
            self.pingTimer = nil
            guard self.active else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + self.retryDelay) {
                guard self.active else { return }
                self.retryDelay = min(self.retryDelay * 2, 30)
                self.openConnection()
            }
        }
    }

    private func updateState(_ newState: ConnectionState) {
        // Must be called on the main queue — all callers ensure this via DispatchQueue.main.async.
        assert(Thread.isMainThread)
        guard state != newState else { return }
        state = newState
        onStateChange?(newState)
    }

    private func startPing() {
        pingTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            self?.send(.ping)
        }
    }
}

// MARK: - URLSessionWebSocketDelegate

extension ResiduumConnection: URLSessionWebSocketDelegate {
    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocol: String?
    ) {
        DispatchQueue.main.async {
            self.retryDelay = 1.0
            self.updateState(.connected)
            self.send(.setVerbose(enabled: true))
            self.startPing()
        }
    }

    func urlSession(
        _ session: URLSession,
        webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode,
        reason: Data?
    ) {
        scheduleReconnect()
    }
}
