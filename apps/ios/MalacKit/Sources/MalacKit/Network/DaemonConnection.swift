import Foundation

/// One WebSocket to a daemon, kept open while started: reconnects with
/// backoff, answers requests by `client_ref`, and hands every message to
/// `onMessage`. The first message after each connect is a snapshot.
@MainActor
public final class DaemonConnection {
    public enum Status: Equatable, Sendable {
        case stopped
        case connecting
        case online
        /// Waiting to retry; `reason` says why the last attempt failed, when known.
        case waiting(until: Date, reason: String?)
        /// The token was refused; retrying will not help.
        case unauthorized
    }

    public private(set) var status = Status.stopped {
        didSet { if status != oldValue { onStatus?(status) } }
    }

    public var onMessage: ((ServerMessage) -> Void)?
    public var onStatus: ((Status) -> Void)?

    public let endpoint: DaemonEndpoint
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private var generation = 0
    private var backoff = Backoff()
    private var retry: Task<Void, Never>?
    private var keepAlive: Task<Void, Never>?
    private var waiters: [String: CheckedContinuation<ServerMessage, Error>] = [:]
    private var nextRef = 1

    public init(endpoint: DaemonEndpoint, session: URLSession = URLSession(configuration: .default)) {
        self.endpoint = endpoint
        self.session = session
    }

    public func start() {
        guard status == .stopped || status == .unauthorized else { return }
        connect()
    }

    public func stop() {
        retry?.cancel()
        disconnect()
        status = .stopped
    }

    /// Connects again right away, e.g. after the network changed under an open socket.
    public func reconnectNow() {
        guard status != .stopped, status != .unauthorized else { return }
        backoff.reset()
        retry?.cancel()
        disconnect()
        connect()
    }

    /// Sends a command without waiting for its reply.
    @discardableResult
    public func send(_ command: Command) -> Bool {
        guard status == .online, let task, let data = try? command.encoded() else { return false }
        task.send(.string(String(decoding: data, as: UTF8.self))) { _ in }
        return true
    }

    /// Sends a command and waits for its reply; an `error` reply throws.
    public func request(_ command: Command, timeout: Duration = .seconds(30)) async throws -> ServerMessage {
        guard status == .online, let task else { throw DaemonError.disconnected }
        let ref = "m\(nextRef)"
        nextRef += 1
        let text = String(decoding: try command.encoded(clientRef: ref), as: UTF8.self)
        let timer = Task { [weak self] in
            try? await Task.sleep(for: timeout)
            self?.resume(ref, with: .failure(DaemonError.timeout))
        }
        defer { timer.cancel() }
        let reply = try await withCheckedThrowingContinuation { continuation in
            waiters[ref] = continuation
            task.send(.string(text)) { error in
                guard error != nil else { return }
                Task { @MainActor [weak self] in self?.resume(ref, with: .failure(DaemonError.disconnected)) }
            }
        }
        if case .error(_, let message) = reply { throw DaemonError.command(message) }
        return reply
    }

    // MARK: Connection

    private func connect() {
        generation += 1
        let current = generation
        status = .connecting
        let task = session.webSocketTask(with: endpoint.request(endpoint.webSocketURL, timeout: 15))
        task.maximumMessageSize = 64 << 20
        self.task = task
        task.resume()
        receive(task, generation: current)
    }

    private func receive(_ task: URLSessionWebSocketTask, generation current: Int) {
        task.receive { [weak self] result in
            Task { @MainActor [weak self] in
                guard let self, self.generation == current else { return }
                switch result {
                case .success(let frame):
                    self.handle(frame)
                    self.receive(task, generation: current)
                case .failure(let error):
                    self.failed(task, error: error)
                }
            }
        }
    }

    private func handle(_ frame: URLSessionWebSocketTask.Message) {
        let data: Data
        switch frame {
        case .string(let text): data = Data(text.utf8)
        case .data(let bytes): data = bytes
        @unknown default: return
        }
        guard let message = try? ServerMessage.decode(data) else { return }
        if case .snapshot = message, status != .online {
            status = .online
            backoff.reset()
            startKeepAlive()
        }
        if let ref = message.clientRef { resume(ref, with: .success(message)) }
        onMessage?(message)
    }

    private func failed(_ task: URLSessionWebSocketTask, error: Error) {
        let code = (task.response as? HTTPURLResponse)?.statusCode
        disconnect()
        if code == 401 {
            status = .unauthorized
            return
        }
        let reason: String? = switch code {
        case 403?: "The daemon refused this device. Remote access may be off (`malac remote`)."
        case .some(let code) where code >= 400: "The daemon answered \(code)."
        default: (error as? URLError).flatMap(Self.describe)
        }
        let delay = backoff.next()
        status = .waiting(until: Date().addingTimeInterval(delay), reason: reason)
        retry = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            self?.connect()
        }
    }

    private func disconnect() {
        generation += 1
        keepAlive?.cancel()
        task?.cancel(with: .goingAway, reason: nil)
        task = nil
        for (ref, _) in waiters { resume(ref, with: .failure(DaemonError.disconnected)) }
    }

    // Pings notice a socket that died with the network before the OS does.
    private func startKeepAlive() {
        keepAlive?.cancel()
        let current = generation
        keepAlive = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(20))
                guard let self, self.generation == current, let task = self.task else { return }
                task.sendPing { error in
                    guard let error else { return }
                    Task { @MainActor [weak self] in
                        guard let self, self.generation == current else { return }
                        self.failed(task, error: error)
                    }
                }
            }
        }
    }

    private func resume(_ ref: String, with result: Result<ServerMessage, Error>) {
        waiters.removeValue(forKey: ref)?.resume(with: result)
    }

    private static func describe(_ error: URLError) -> String? {
        switch error.code {
        case .notConnectedToInternet, .networkConnectionLost: "No network connection."
        case .cannotConnectToHost, .cannotFindHost, .timedOut: "The daemon cannot be reached."
        case .secureConnectionFailed, .serverCertificateUntrusted: "The secure connection failed."
        case .cancelled: nil
        default: error.localizedDescription
        }
    }
}

/// 0.5 s, doubling up to 10 s, like the web UI.
struct Backoff {
    private static let first: TimeInterval = 0.5
    private static let limit: TimeInterval = 10
    private var current = first

    mutating func next() -> TimeInterval {
        defer { current = min(current * 2, Self.limit) }
        return current
    }

    mutating func reset() {
        current = Self.first
    }
}
