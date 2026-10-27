import MalacKit
import QuickLook
import SwiftUI

/// What an agent sent with `send_to_user`, shown as the files themselves:
/// images inline (tap for the full-screen viewer), other files as a card
/// that downloads them and opens them with Quick Look.
struct SharedFilesCard: View {
    let shared: SharedFiles
    let session: DaemonSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Sent to you", systemImage: "paperplane")
                .font(.caption.weight(.semibold))
                .foregroundStyle(Palette.text2)
            if let caption = shared.caption {
                Text(caption).font(.callout).textSelection(.enabled)
            }
            ForEach(shared.files) { file in
                if file.isImage {
                    SharedImage(file: file, api: session.api)
                } else {
                    SharedFileRow(file: file, api: session.api)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(Palette.surface)
        .accessibilityElement(children: .contain)
    }
}

/// Shared files fetched while the app runs, so scrolling back shows them at once.
@MainActor
final class SharedFileCache {
    static let shared = SharedFileCache()

    final class Picture {
        let data: Data
        let mediaType: String
        /// Small enough to draw in a list.
        let preview: UIImage

        init(data: Data, mediaType: String, preview: UIImage) {
            self.data = data
            self.mediaType = mediaType
            self.preview = preview
        }
    }

    private let pictures: NSCache<NSString, Picture> = {
        let cache = NSCache<NSString, Picture>()
        cache.totalCostLimit = 64 << 20
        return cache
    }()

    func picture(_ file: SharedFile, api: DaemonAPI) async throws -> Picture {
        let key = Self.key(file, api: api)
        if let cached = pictures.object(forKey: key) { return cached }
        let (data, mediaType) = try await api.shared(file.id)
        guard let image = UIImage(data: data) else { throw DaemonError.invalidResponse }
        let preview = await image.byPreparingThumbnail(ofSize: Self.fit(image.size, in: CGSize(width: 1200, height: 900))) ?? image
        let picture = Picture(data: data, mediaType: mediaType, preview: preview)
        pictures.setObject(picture, forKey: key, cost: data.count)
        return picture
    }

    /// The file on disk under its own name, for Quick Look; downloaded once.
    func download(_ file: SharedFile, api: DaemonAPI) async throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appending(path: "Shared/\(file.id)")
        let url = folder.appending(path: file.name.isEmpty ? "file" : file.name)
        if FileManager.default.fileExists(atPath: url.path) { return url }
        let (data, _) = try await api.shared(file.id, download: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
        return url
    }

    private static func key(_ file: SharedFile, api: DaemonAPI) -> NSString {
        "\(api.endpoint.base.absoluteString)\(file.id)" as NSString
    }

    private static func fit(_ size: CGSize, in box: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0 else { return box }
        let scale = min(1, box.width / size.width, box.height / size.height)
        return CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
    }

    /// What to tell the user when a shared file cannot be fetched.
    static func describe(_ error: Error) -> String {
        if case DaemonError.http(404, _) = error {
            return "The daemon no longer has this file; shared files are kept for 30 days."
        }
        return error.localizedDescription
    }
}

/// A shared image at a fixed height, so the transcript does not jump when it loads.
private struct SharedImage: View {
    let file: SharedFile
    let api: DaemonAPI
    @State private var picture: SharedFileCache.Picture?
    @State private var error: String?
    @State private var attempt = 0
    @State private var viewing: ViewerImage?

    private static let height: CGFloat = 220

    var body: some View {
        Group {
            if let picture {
                Button {
                    let full = UIImage(data: picture.data) ?? picture.preview
                    viewing = ViewerImage(image: full, data: picture.data, name: file.name, mediaType: picture.mediaType)
                } label: {
                    Image(uiImage: picture.preview)
                        .resizable()
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous).strokeBorder(Palette.border))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(file.name)
                .accessibilityHint("Opens full screen")
            } else if let error {
                VStack(alignment: .leading, spacing: 6) {
                    Label("Could not load \(file.name)", systemImage: "photo")
                        .font(.footnote.weight(.medium))
                    Text(error).font(.caption).foregroundStyle(Palette.red)
                    Button("Try again") { attempt += 1 }
                        .font(.footnote.weight(.semibold))
                        .tint(Palette.green)
                }
                .padding(10)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .card()
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .card()
                    .accessibilityLabel("Loading \(file.name)")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: Self.height, alignment: .leading)
        .task(id: attempt) {
            guard picture == nil else { return }
            error = nil
            do {
                picture = try await SharedFileCache.shared.picture(file, api: api)
            } catch is CancellationError {
            } catch {
                self.error = SharedFileCache.describe(error)
            }
        }
        .imageViewer($viewing)
    }
}

/// A file that is not an image: its name, size and type; tap to open it.
private struct SharedFileRow: View {
    let file: SharedFile
    let api: DaemonAPI
    @State private var downloading = false
    @State private var preview: URL?
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button { Task { await open() } } label: {
                HStack(spacing: 10) {
                    Image(systemName: Self.symbol(file.mediaType))
                        .font(.title3)
                        .frame(width: 28)
                        .foregroundStyle(Palette.text2)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(file.name).font(.callout.weight(.medium)).lineLimit(2)
                        Text(detail).font(.caption).foregroundStyle(Palette.text3)
                    }
                    Spacer(minLength: 8)
                    if downloading {
                        ProgressView()
                    } else {
                        Image(systemName: "arrow.down.circle").foregroundStyle(Palette.text2).accessibilityHidden(true)
                    }
                }
                .padding(10)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(downloading)
            .card(Palette.surface2)
            .accessibilityLabel("\(file.name), \(detail)")
            .accessibilityHint("Downloads and opens the file")
            if let error {
                Text(error).font(.caption).foregroundStyle(Palette.red)
            }
        }
        .quickLookPreview($preview)
    }

    private var detail: String {
        let size = ByteCountFormatter.string(fromByteCount: Int64(file.size), countStyle: .file)
        return file.size > 0 ? "\(size) · \(file.mediaType)" : file.mediaType
    }

    private func open() async {
        downloading = true
        error = nil
        defer { downloading = false }
        do {
            preview = try await SharedFileCache.shared.download(file, api: api)
        } catch {
            self.error = "Could not download it: \(SharedFileCache.describe(error))"
        }
    }

    private static func symbol(_ type: String) -> String {
        switch type {
        case "application/pdf": "doc.richtext"
        case "application/zip", "application/gzip", "application/x-tar": "doc.zipper"
        case "image/svg+xml": "photo"
        case let type where type.hasPrefix("video/"): "film"
        case let type where type.hasPrefix("audio/"): "waveform"
        case let type where type.hasPrefix("text/") || type.hasSuffix("json") || type.hasSuffix("xml"): "doc.text"
        default: "doc"
        }
    }
}
