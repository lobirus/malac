import Foundation

/// A conversation message, as saved in transcripts and sent in `assistant_message`.
public struct Message: Hashable, Sendable, Decodable {
    public var role: Role
    public var content: [Block]

    public init(role: Role, content: [Block]) {
        self.role = role
        self.content = content
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        role = c.value("role", default: .assistant)
        content = c.value("content", default: [])
    }
}

public struct ImageBlock: Hashable, Sendable {
    public var mediaType: String
    /// Base64.
    public var data: String

    public init(mediaType: String, data: String) {
        self.mediaType = mediaType
        self.data = data
    }
}

public enum Block: Hashable, Sendable {
    case text(String)
    case image(ImageBlock)
    case thinking(String)
    case toolUse(id: String, name: String, input: JSONValue)
    case toolResult(toolUseId: String, content: [Block], isError: Bool)
    case unknown(type: String)

    /// The text of a tool result, with images as placeholders the way the daemon previews them.
    public static func resultText(_ content: [Block]) -> String {
        content.map { block in
            switch block {
            case .text(let text): text
            case .image(let image): "[\(image.mediaType) image]"
            default: ""
            }
        }.joined()
    }
}

extension Block: Decodable {
    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        let type: String = c.value("type", default: "")
        switch type {
        case "text":
            self = .text(c.value("text", default: ""))
        case "image":
            self = .image(ImageBlock(mediaType: c.value("media_type", default: ""), data: c.value("data", default: "")))
        case "thinking":
            self = .thinking(c.value("text", default: ""))
        case "tool_use":
            self = .toolUse(id: c.value("id", default: ""), name: c.value("name", default: ""), input: c.value("input", default: .null))
        case "tool_result":
            self = .toolResult(
                toolUseId: c.value("tool_use_id", default: ""),
                content: c.value("content", default: []),
                isError: c.value("is_error", default: false)
            )
        default:
            self = .unknown(type: type)
        }
    }
}
