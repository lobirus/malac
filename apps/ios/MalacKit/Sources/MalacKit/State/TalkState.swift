import Foundation

/// A talk mode conversation as the talk view shows it: what is happening
/// and the lines said so far.
public struct TalkState: Sendable {
    public enum Phase: Hashable, Sendable {
        case connecting, listening, thinking, speaking, ended

        public var label: String {
            switch self {
            case .connecting: "Connecting"
            case .listening: "Listening"
            case .thinking: "Thinking"
            case .speaking: "Speaking"
            case .ended: "Ended"
            }
        }
    }

    public struct Line: Identifiable, Hashable, Sendable {
        public enum Kind: Hashable, Sendable { case user, voice, tool, error }

        public var id: Int
        public var kind: Kind
        public var text: String
        /// Voice lines grow with deltas until their final transcript.
        public var final: Bool
    }

    public static let lineLimit = 200

    public private(set) var ready = false
    public private(set) var model: String?
    public private(set) var voice: String?
    public private(set) var lines: [Line] = []
    /// Why the conversation ended; nil while it goes on.
    public private(set) var endReason: String?
    private var thinking = false
    private var counter = 0

    /// Set by the player while the voice's audio plays.
    public var playing = false

    public init() {}

    public var phase: Phase {
        if endReason != nil { return .ended }
        if !ready { return .connecting }
        if playing { return .speaking }
        return thinking ? .thinking : .listening
    }

    public mutating func apply(_ event: TalkEvent) {
        switch event {
        case let .ready(model, voice):
            ready = true
            self.model = model
            self.voice = voice
        case .listening:
            thinking = false
            playing = false
        case .thinking:
            thinking = true
        case let .transcript(role, text, final):
            if role == .user {
                if final { append(.user, text) }
            } else {
                voiceSaid(text, final: final)
            }
        case let .tool(name, summary):
            append(.tool, summary.isEmpty ? name : summary)
        case .done:
            thinking = false
            if let index = openVoiceLine { lines[index].final = true }
        case .error(let message):
            append(.error, message)
        case .closed(let reason):
            endReason = reason.isEmpty ? "Talk mode ended." : reason
            thinking = false
            playing = false
        case .unknown:
            break
        }
    }

    /// Typed input shows like a spoken line.
    public mutating func typed(_ text: String) {
        append(.user, text)
    }

    /// The conversation ended here, e.g. with the end button.
    public mutating func end(_ reason: String) {
        if endReason == nil { apply(.closed(reason)) }
    }

    private var openVoiceLine: Int? {
        lines.lastIndex { $0.kind == .voice && !$0.final }
    }

    private mutating func voiceSaid(_ text: String, final: Bool) {
        if let index = openVoiceLine {
            if final {
                if !text.isEmpty { lines[index].text = text }
                lines[index].final = true
            } else {
                lines[index].text += text
            }
            return
        }
        guard !text.isEmpty else { return }
        append(.voice, text, final: final)
    }

    private mutating func append(_ kind: Line.Kind, _ text: String, final: Bool = true) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !final else { return }
        counter += 1
        lines.append(Line(id: counter, kind: kind, text: final ? trimmed : text, final: final))
        if lines.count > Self.lineLimit { lines.removeFirst(lines.count - Self.lineLimit) }
    }
}
