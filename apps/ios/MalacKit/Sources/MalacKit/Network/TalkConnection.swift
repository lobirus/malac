import Foundation

/// A text frame from talk mode's socket.
public enum TalkEvent: Hashable, Sendable {
    /// Start streaming the microphone.
    case ready(model: String?, voice: String?)
    /// The user started speaking: stop playing at once.
    case listening
    /// The user stopped speaking.
    case thinking
    /// What the user (`final` only) or the voice said; assistant deltas are not final.
    case transcript(role: Role, text: String, final: Bool)
    /// The voice model asked malac something, checked on it, answered an approval or stopped it.
    case tool(name: String, summary: String)
    /// A reply finished.
    case done
    case error(String)
    /// The conversation ended.
    case closed(String)
    case unknown(type: String)

    public static func decode(_ data: Data) -> TalkEvent? {
        guard let message = try? JSONDecoder().decode(JSONValue.self, from: data), let type = message["type"]?.stringValue else { return nil }
        switch type {
        case "ready":
            return .ready(model: message["model"]?.stringValue, voice: message["voice"]?.stringValue)
        case "listening":
            return .listening
        case "thinking":
            return .thinking
        case "transcript":
            return .transcript(
                role: message["role"]?.stringValue.flatMap(Role.init(rawValue:)) ?? .assistant,
                text: message["text"]?.stringValue ?? "",
                final: message["final"]?.boolValue ?? true
            )
        case "tool":
            return .tool(name: message["name"]?.stringValue ?? "", summary: message["summary"]?.stringValue ?? "")
        case "done":
            return .done
        case "error":
            return .error(message["message"]?.stringValue ?? "Talk mode failed.")
        case "closed":
            return .closed(message["reason"]?.stringValue ?? "")
        default:
            return .unknown(type: type)
        }
    }
}

/// A text frame for talk mode's socket.
public enum TalkCommand: Hashable, Sendable {
    /// Typed input, as if spoken.
    case text(String)
    /// Stop the current reply.
    case interrupt
    /// End talk mode.
    case stop

    public var json: String {
        let object: JSONValue = switch self {
        case .text(let text): .object(["type": .string("text"), "text": .string(text)])
        case .interrupt: .object(["type": .string("interrupt")])
        case .stop: .object(["type": .string("stop")])
        }
        return object.compactText
    }
}

/// Talk mode's WebSocket (`/api/voice/talk`): binary frames both ways are
/// 16-bit little-endian mono PCM at 24 kHz, text frames are JSON. The token
/// goes in a header, as on the main socket.
@MainActor
public final class TalkConnection {
    public var onEvent: ((TalkEvent) -> Void)?
    /// The voice's audio, in the order it arrived.
    public var onAudio: ((Data) -> Void)?

    private let url: URL
    private let endpoint: DaemonEndpoint
    private let session: URLSession
    private var task: URLSessionWebSocketTask?
    private var ended = false

    public init(endpoint: DaemonEndpoint, tab: String?, agent: String?, session: URLSession = URLSession(configuration: .default)) {
        self.endpoint = endpoint
        self.session = session
        url = endpoint.talkURL(tab: tab, agent: agent)
    }

    public func start() {
        guard task == nil, !ended else { return }
        let task = session.webSocketTask(with: endpoint.request(url, timeout: 15))
        task.maximumMessageSize = 16 << 20
        self.task = task
        task.resume()
        receive(task)
    }

    /// About 100 ms of microphone audio.
    public func send(audio: Data) {
        guard let task, !ended else { return }
        task.send(.data(audio)) { _ in }
    }

    public func send(_ command: TalkCommand) {
        guard let task, !ended else { return }
        task.send(.string(command.json)) { _ in }
    }

    /// Ends talk mode: says `stop`, then closes the socket.
    public func close() {
        guard let task, !ended else { return }
        ended = true
        task.send(.string(TalkCommand.stop.json)) { _ in
            task.cancel(with: .normalClosure, reason: nil)
        }
    }

    private func receive(_ task: URLSessionWebSocketTask) {
        task.receive { [weak self] result in
            Task { @MainActor [weak self] in
                guard let self, self.task === task, !self.ended else { return }
                switch result {
                case .success(.data(let data)):
                    self.onAudio?(data)
                case .success(.string(let text)):
                    if let event = TalkEvent.decode(Data(text.utf8)) { self.emit(event) }
                case .success:
                    break
                case .failure(let error):
                    self.failed(task, error: error)
                    return
                }
                if !self.ended { self.receive(task) }
            }
        }
    }

    private func emit(_ event: TalkEvent) {
        if case .closed = event {
            ended = true
            task?.cancel(with: .normalClosure, reason: nil)
        }
        onEvent?(event)
    }

    private func failed(_ task: URLSessionWebSocketTask, error: Error) {
        let reason: String = switch (task.response as? HTTPURLResponse)?.statusCode {
        case 401?: DaemonError.unauthorized.localizedDescription
        case 403?: "The daemon refused talk mode from this device."
        case 404?: "This daemon has no talk mode. Update malac on the computer."
        case .some(let code) where code >= 400: "The daemon answered \(code)."
        default: (error as? URLError)?.code == .cancelled ? "Talk mode ended." : "The connection to the daemon was lost."
        }
        emit(.closed(reason))
    }
}
