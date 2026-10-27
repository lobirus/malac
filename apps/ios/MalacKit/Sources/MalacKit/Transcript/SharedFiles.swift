import Foundation

/// A file an agent sent with `send_to_user`, kept by the daemon under `GET /api/shared/{id}`.
public struct SharedFile: Identifiable, Hashable, Sendable {
    public var id: String
    public var name: String
    public var mediaType: String
    /// In bytes.
    public var size: Int

    public init(id: String, name: String, mediaType: String, size: Int) {
        self.id = id
        self.name = name
        self.mediaType = mediaType
        self.size = size
    }

    /// A picture the app can draw inline; SVG opens as a file.
    public var isImage: Bool { mediaType.hasPrefix("image/") && mediaType != "image/svg+xml" }
}

/// The files of a `send_to_user` result and their caption.
public struct SharedFiles: Hashable, Sendable {
    public var caption: String?
    public var files: [SharedFile]

    public init(caption: String?, files: [SharedFile]) {
        self.caption = caption
        self.files = files
    }

    /// The tags in a tool result: an optional `<caption>…</caption>` and one
    /// `<shared id="…" name="…" type="…" size="…"/>` per file, as
    /// `crates/malac-core/src/shared.rs` writes and parses them. Nil when the
    /// text names no file.
    public static func parse(_ text: String) -> SharedFiles? {
        var files: [SharedFile] = []
        var rest = text[...]
        while let start = rest.range(of: "<shared ") {
            let tail = rest[start.lowerBound...]
            guard let end = tail.range(of: "/>") else { break }
            let tag = tail[..<end.lowerBound]
            if let id = attribute("id", in: tag), let name = attribute("name", in: tag) {
                files.append(SharedFile(
                    id: id,
                    name: name,
                    mediaType: attribute("type", in: tag) ?? "application/octet-stream",
                    size: attribute("size", in: tag).flatMap { Int($0) } ?? 0
                ))
            }
            rest = tail[end.upperBound...]
        }
        guard !files.isEmpty else { return nil }
        var caption: String?
        if let open = text.range(of: "<caption>"), let close = text.range(of: "</caption>", range: open.upperBound ..< text.endIndex) {
            let value = text[open.upperBound ..< close.lowerBound].trimmingCharacters(in: .whitespacesAndNewlines)
            caption = value.isEmpty ? nil : value
        }
        return SharedFiles(caption: caption, files: files)
    }

    private static func attribute(_ key: String, in tag: Substring) -> String? {
        guard let start = tag.range(of: "\(key)=\"") else { return nil }
        let value = tag[start.upperBound...]
        guard let end = value.firstIndex(of: "\"") else { return nil }
        return String(value[..<end])
    }
}
