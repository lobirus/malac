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
    public var webSocketURL: URL { webSocket(url("api/ws")) }

    /// Talk mode's socket, through the tab's active agent or the one named.
    public func talkURL(tab: String?, agent: String?) -> URL {
        var query: [URLQueryItem] = []
        if let tab { query.append(URLQueryItem(name: "tab", value: tab)) }
        if let agent { query.append(URLQueryItem(name: "agent", value: agent)) }
        return webSocket(url("api/voice/talk", query: query))
    }

    private func webSocket(_ url: URL) -> URL {
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
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
        let (data, _) = try await body(request, session: session)
        do {
            return try JSONDecoder.malac().decode(T.self, from: data)
        } catch {
            throw DaemonError.invalidResponse
        }
    }

    /// The body of a successful response, with its type.
    static func body(_ request: URLRequest, session: URLSession) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw DaemonError.invalidResponse }
        guard (200 ..< 300).contains(http.statusCode) else {
            throw error(status: http.statusCode, body: data)
        }
        return (data, http)
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

    // MARK: Voice and shared files

    /// What dictation, speech and talk mode offer; `refresh` reads the providers' model lists again first.
    public func voice(refresh: Bool = false) async throws -> VoiceStatus {
        try await get("api/voice", query: refresh ? [URLQueryItem(name: "refresh", value: "true")] : [])
    }

    /// Changes `[voice]` settings; the daemon broadcasts the result as `voice` too.
    public func setVoice(_ changes: [VoiceSetting]) async throws -> VoiceStatus {
        var request = endpoint.request(endpoint.url("api/voice"))
        request.httpMethod = "PUT"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try VoiceSetting.body(changes)
        return try await HTTP.send(request, session: session, as: VoiceStatus.self)
    }

    /// Dictation: a recording (16 kHz mono 16-bit WAV) as text.
    public func transcribe(wav: Data, language: String? = nil) async throws -> Transcription {
        let query = language.map { [URLQueryItem(name: "language", value: $0)] } ?? []
        // Transcription can take a while for a long recording.
        var request = endpoint.request(endpoint.url("api/voice/transcribe", query: query), timeout: 120)
        request.httpMethod = "POST"
        request.setValue("audio/wav", forHTTPHeaderField: "Content-Type")
        request.httpBody = wav
        return try await HTTP.send(request, session: session, as: Transcription.self)
    }

    /// The audio of a `say` broadcast (`audio/mpeg` or `audio/wav`).
    public func spoken(_ id: String) async throws -> (data: Data, mediaType: String) {
        let (data, response) = try await HTTP.body(endpoint.request(endpoint.url("api/voice/say/\(id)"), timeout: 60), session: session)
        return (data, Self.mediaType(response) ?? "audio/mpeg")
    }

    /// A file an agent sent with `send_to_user`. `download` asks for HTML and
    /// SVG as themselves rather than as text.
    public func shared(_ id: String, download: Bool = false) async throws -> (data: Data, mediaType: String) {
        let query = download ? [URLQueryItem(name: "download", value: "true")] : []
        let (data, response) = try await HTTP.body(endpoint.request(endpoint.url("api/shared/\(id)", query: query), timeout: 120), session: session)
        return (data, Self.mediaType(response) ?? "application/octet-stream")
    }

    private static func mediaType(_ response: HTTPURLResponse) -> String? {
        (response.value(forHTTPHeaderField: "Content-Type")?.split(separator: ";").first)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    private func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        try await HTTP.send(endpoint.request(endpoint.url(path, query: query)), session: session, as: T.self)
    }
}
