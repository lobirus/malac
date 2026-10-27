import Foundation

/// One voice feature: dictation, speech or talk mode.
public struct VoiceFeature: Hashable, Sendable, Decodable {
    /// The setting.
    public var enabled: Bool
    /// The model in use: `configured`, else the first of `models`; nil when nothing can do it.
    public var model: String?
    /// The model the settings name, if any.
    public var configured: String?
    /// The voice, for speech and talk mode.
    public var voice: String?
    /// Every `provider/model` that can do it; speech may list `system`.
    public var models: [String]

    public static let unavailable = VoiceFeature(enabled: false, model: nil, configured: nil, voice: nil, models: [])

    public init(enabled: Bool, model: String?, configured: String?, voice: String?, models: [String]) {
        self.enabled = enabled
        self.model = model
        self.configured = configured
        self.voice = voice
        self.models = models
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        self.init(
            enabled: c.value("enabled", default: false),
            model: c.optional("model"),
            configured: c.optional("configured"),
            voice: c.optional("voice"),
            models: c.value("models", default: [])
        )
    }

    /// On and with a model: the app shows the feature's button only then.
    public var isReady: Bool { enabled && model != nil }
}

/// What dictation, speech and talk mode offer, from the snapshot, `voice`
/// broadcasts and `GET /api/voice`.
public struct VoiceStatus: Hashable, Sendable, Decodable {
    public var dictation: VoiceFeature
    public var speech: VoiceFeature
    public var talk: VoiceFeature
    /// The dictation language; nil lets the model work it out.
    public var language: String?
    /// The voices speech and talk models know.
    public var voices: [String]
    /// When the providers' model lists were last read.
    public var checked: Date?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        dictation = c.value("dictation", default: .unavailable)
        speech = c.value("speech", default: .unavailable)
        talk = c.value("talk", default: .unavailable)
        language = c.optional("language")
        voices = c.value("voices", default: [])
        checked = c.optional("checked")
    }
}

/// A `say` broadcast: a sentence to read aloud on the connection whose listener id it names.
public struct SpokenLine: Hashable, Sendable {
    /// Fetch the audio from `GET /api/voice/say/{id}`.
    public var id: String
    public var text: String
    /// `approval`, `agent` (its `say` tool), `finished` or `failed`.
    public var reason: String
    public var tab: String?
    public var agent: String?
    public var listener: Int

    public init(id: String, text: String, reason: String, tab: String?, agent: String?, listener: Int) {
        self.id = id
        self.text = text
        self.reason = reason
        self.tab = tab
        self.agent = agent
        self.listener = listener
    }
}

/// One change for `PUT /api/voice`; a nil value removes the setting.
public enum VoiceSetting: Hashable, Sendable {
    case dictation(Bool)
    case dictationModel(String?)
    case language(String?)
    case speech(Bool)
    case speechModel(String?)
    case speechVoice(String?)
    case talk(Bool)
    case talkModel(String?)
    case talkVoice(String?)

    public var key: String {
        switch self {
        case .dictation: "dictation"
        case .dictationModel: "dictation_model"
        case .language: "language"
        case .speech: "speech"
        case .speechModel: "speech_model"
        case .speechVoice: "speech_voice"
        case .talk: "talk"
        case .talkModel: "talk_model"
        case .talkVoice: "talk_voice"
        }
    }

    public var value: JSONValue {
        switch self {
        case .dictation(let on), .speech(let on), .talk(let on):
            .bool(on)
        case .dictationModel(let text), .language(let text), .speechModel(let text), .speechVoice(let text),
             .talkModel(let text), .talkVoice(let text):
            text.map { $0.isEmpty ? .null : .string($0) } ?? .null
        }
    }

    /// The JSON body of `PUT /api/voice`.
    public static func body(_ changes: [VoiceSetting]) throws -> Data {
        let object = JSONValue.object(Dictionary(changes.map { ($0.key, $0.value) }, uniquingKeysWith: { _, last in last }))
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(object)
    }
}

/// `POST /api/voice/transcribe`: what was said, and the model that heard it.
public struct Transcription: Hashable, Sendable, Decodable {
    public var text: String
    public var model: String

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        text = c.value("text", default: "")
        model = c.value("model", default: "")
    }
}
