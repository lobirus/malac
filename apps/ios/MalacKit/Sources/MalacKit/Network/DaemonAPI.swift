import Foundation

/// Where a daemon is and how to authenticate to it.
public struct DaemonEndpoint: Hashable, Sendable {
    /// The daemon's address, ending in `/`.
    public var base: URL
    public var token: String

    public init(base: URL, token: String) {
        self.base = base
        self.token = token
    }

    public func url(_ path: String, query: [URLQueryItem] = []) -> URL {
        let url = base.appending(path: path)
        return query.isEmpty ? url : url.appending(queryItems: query)
    }

    /// `ws://…/api/ws`, or `wss://` behind TLS. The token goes in a header, not the URL.
    public var webSocketURL: URL {
        var components = URLComponents(url: url("api/ws"), resolvingAgainstBaseURL: false)!
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        return components.url!
    }

    public func request(_ url: URL, timeout: TimeInterval = 20) -> URLRequest {
        var request = URLRequest(url: url, timeoutInterval: timeout)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }
}

public enum DaemonError: LocalizedError, Equatable {
    /// The token is missing, wrong or revoked: pair again.
    case unauthorized
    /// The daemon refused the request, e.g. because remote access is off.
    case forbidden(String)
    case http(status: Int, message: String)
    /// The daemon answered a command with `error`.
    case command(String)
    case disconnected
    case timeout
    case invalidResponse

    public var errorDescription: String? {
        switch self {
        case .unauthorized: "This device is not paired with the daemon any more. Pair it again."
        case .forbidden(let message): message.isEmpty ? "The daemon refused the connection." : message
        case let .http(status, message): message.isEmpty ? "The daemon answered \(status)." : message
        case .command(let message): message
        case .disconnected: "Not connected to the daemon."
        case .timeout: "The daemon did not answer in time."
        case .invalidResponse: "The daemon sent something this app does not understand."
        }
    }
}

enum HTTP {
    static func send<T: Decodable>(_ request: URLRequest, session: URLSession, as type: T.Type) async throws -> T {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw DaemonError.invalidResponse }
        guard (200 ..< 300).contains(http.statusCode) else {
            throw error(status: http.statusCode, body: data)
        }
        do {
            return try JSONDecoder.malac().decode(T.self, from: data)
        } catch {
            throw DaemonError.invalidResponse
        }
    }

    /// Failures are `{"error": "…"}` or plain text.
    static func error(status: Int, body: Data) -> DaemonError {
        let message = (try? JSONDecoder().decode([String: String].self, from: body))?["error"]
            ?? String(decoding: body, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        switch status {
        case 401: return .unauthorized
        case 403: return .forbidden(message)
        default: return .http(status: status, message: message)
        }
    }
}

/// The daemon's REST endpoints this app uses.
public struct DaemonAPI: Sendable {
    public let endpoint: DaemonEndpoint
    private let session: URLSession

    public init(endpoint: DaemonEndpoint, session: URLSession = .shared) {
        self.endpoint = endpoint
        self.session = session
    }

    /// A saved transcript. A brand-new agent may have none yet (404).
    public func transcript(_ agent: String) async throws -> SessionTranscript {
        try await get("api/sessions/\(agent)")
    }

    public func sessions(limit: Int = 50) async throws -> [SessionSummary] {
        struct List: Decodable { var sessions: [SessionSummary] }
        let list: List = try await get("api/sessions", query: [URLQueryItem(name: "limit", value: String(limit))])
        return list.sessions
    }

    /// The snapshot, as on the WebSocket; for checks in the background.
    public func snapshot() async throws -> Snapshot {
        try await get("api/snapshot")
    }

    /// Paired devices; this one has `current` set.
    public func devices() async throws -> [DeviceView] {
        struct List: Decodable { var devices: [DeviceView] }
        let list: List = try await get("api/devices")
        return list.devices
    }

    /// Revokes a device's token; revoking this one unpairs it.
    public func revoke(device id: String) async throws {
        var request = endpoint.request(endpoint.url("api/devices/\(id)"))
        request.httpMethod = "DELETE"
        struct Revoked: Decodable {}
        _ = try await HTTP.send(request, session: session, as: Revoked.self)
    }

    /// Runs a command without a WebSocket, e.g. from a notification action.
    public func command(_ command: Command) async throws -> ServerMessage {
        var request = endpoint.request(endpoint.url("api/command"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try command.encoded()
        let reply = try await HTTP.send(request, session: session, as: ServerMessage.self)
        if case .error(_, let message) = reply { throw DaemonError.command(message) }
        return reply
    }

    private func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        try await HTTP.send(endpoint.request(endpoint.url(path, query: query)), session: session, as: T.self)
    }
}
