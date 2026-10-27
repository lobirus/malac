import MalacKit
import Photos
import SwiftUI
import UIKit

/// An image for the full-screen viewer: the picture, and its original bytes for saving and sharing.
struct ViewerImage: Identifiable {
    let id = UUID()
    let image: UIImage
    var data: Data?
    var name: String
    var mediaType: String

    init(image: UIImage, data: Data?, name: String, mediaType: String) {
        self.image = image
        self.data = data
        self.name = name
        self.mediaType = mediaType
    }

    /// An image of a message or tool result.
    init?(block: ImageBlock, name: String) {
        guard let data = Data(base64Encoded: block.data), let image = UIImage(data: data) else { return nil }
        self.init(image: image, data: data, name: name, mediaType: block.mediaType)
    }

    /// A file name with the extension its type implies.
    var fileName: String {
        let ext = switch mediaType {
        case "image/png": "png"
        case "image/gif": "gif"
        case "image/webp": "webp"
        case "image/heic": "heic"
        default: "jpg"
        }
        let base = name.replacingOccurrences(of: "/", with: "-")
        return base.contains(".") ? base : "\(base).\(ext)"
    }
}

extension View {
    /// Shows the image full screen while `item` is set.
    func imageViewer(_ item: Binding<ViewerImage?>) -> some View {
        fullScreenCover(item: item) { ImageViewer(item: $0) }
    }
}

/// Every image in the app opens here: pinch or double-tap to zoom, drag to
/// pan, swipe down or Esc to close, and share or save it.
struct ImageViewer: View {
    let item: ViewerImage
    @Environment(\.dismiss) private var dismiss
    @State private var zoom = ZoomController()
    @State private var drag: CGFloat = 0
    @State private var file: URL?
    @State private var saving = false
    @State private var message: String?

    var body: some View {
        ZStack {
            Color.black.opacity(Double(1 - drag * 0.7)).ignoresSafeArea()
            ZoomableImage(image: item.image, controller: zoom, onDrag: { drag = $0 }, onDismiss: { dismiss() })
                .ignoresSafeArea()
                .accessibilityElement()
                .accessibilityLabel(item.name)
                .accessibilityAddTraits(.isImage)
                .accessibilityHint("Pinch or double-tap to zoom, swipe down to close.")
                .accessibilityZoomAction { action in
                    if action.direction == .zoomIn { zoom.zoomIn() } else { zoom.zoomOut() }
                }
                .accessibilityAction(.escape) { dismiss() }
        }
        .overlay(alignment: .top) { bar.opacity(Double(1 - drag)) }
        .overlay(alignment: .bottom) {
            if let message {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(.white)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                    .background(Color(white: 0.13), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Color(white: 0.2)))
                    .padding(.horizontal, 16)
                    .padding(.bottom, 24)
                    .onTapGesture { self.message = nil }
                    .task(id: message) {
                        try? await Task.sleep(for: .seconds(4))
                        if self.message == message { self.message = nil }
                    }
            }
        }
        .background { shortcuts }
        .environment(\.colorScheme, .dark)
        .statusBarHidden()
        .task { file = writeFile() }
    }

    private var bar: some View {
        HStack(spacing: 0) {
            Button { dismiss() } label: {
                Image(systemName: "xmark").frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .keyboardShortcut(.cancelAction)
            .accessibilityLabel("Close")
            Text(item.name)
                .font(.subheadline)
                .lineLimit(1)
                .truncationMode(.middle)
                .padding(.horizontal, 4)
                .accessibilityHidden(true)
            Spacer(minLength: 8)
            if let file {
                ShareLink(item: file, preview: SharePreview(item.name, image: Image(uiImage: item.image))) {
                    Image(systemName: "square.and.arrow.up").frame(width: 44, height: 44).contentShape(Rectangle())
                }
                .accessibilityLabel("Share")
            }
            Button { Task { await save() } } label: {
                Group {
                    if saving { ProgressView() } else { Image(systemName: "square.and.arrow.down") }
                }
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
            }
            .disabled(saving)
            .accessibilityLabel("Save to Photos")
        }
        .font(.body.weight(.medium))
        .foregroundStyle(.white)
        .padding(.horizontal, 8)
    }

    /// Keyboard zoom, for an iPad with a keyboard.
    private var shortcuts: some View {
        Group {
            Button("Zoom in") { zoom.zoomIn() }.keyboardShortcut("=", modifiers: .command)
            Button("Zoom out") { zoom.zoomOut() }.keyboardShortcut("-", modifiers: .command)
            Button("Fit") { zoom.reset() }.keyboardShortcut("0", modifiers: .command)
        }
        .opacity(0)
        .accessibilityHidden(true)
    }

    /// The original bytes as a file with its name, for sharing in their own format.
    private func writeFile() -> URL? {
        let data = item.data ?? item.image.pngData()
        guard let data else { return nil }
        let folder = FileManager.default.temporaryDirectory.appending(path: "Viewer/\(item.id.uuidString)")
        let url = folder.appending(path: item.data == nil ? "\(item.name).png" : item.fileName)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try data.write(to: url)
            return url
        } catch {
            return nil
        }
    }

    private func save() async {
        saving = true
        defer { saving = false }
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else {
            message = "malac may not add photos. Allow it in Settings → Privacy & Security → Photos."
            return
        }
        let data = item.data ?? item.image.pngData()
        do {
            try await PHPhotoLibrary.shared().performChanges {
                let request = PHAssetCreationRequest.forAsset()
                if let data {
                    request.addResource(with: .photo, data: data, options: nil)
                }
            }
            message = "Saved to Photos"
        } catch {
            message = "Could not save the image: \(error.localizedDescription)"
        }
    }
}

/// Zooms the viewer from SwiftUI: VoiceOver's zoom action and keyboard shortcuts.
@MainActor
final class ZoomController {
    fileprivate weak var view: ZoomingScrollView?

    func zoomIn() { view?.step(1.6) }
    func zoomOut() { view?.step(1 / 1.6) }
    func reset() { view?.fit(animated: true) }
}

private struct ZoomableImage: UIViewRepresentable {
    let image: UIImage
    let controller: ZoomController
    let onDrag: (CGFloat) -> Void
    let onDismiss: () -> Void

    func makeUIView(context: Context) -> ZoomingScrollView {
        let view = ZoomingScrollView(image: image)
        controller.view = view
        return view
    }

    func updateUIView(_ view: ZoomingScrollView, context: Context) {
        view.onDrag = onDrag
        view.onDismiss = onDismiss
    }
}

/// A scroll view that zooms its image: pinch, double-tap, and a downward
/// swipe that closes the viewer while the image is not zoomed in.
private final class ZoomingScrollView: UIScrollView, UIScrollViewDelegate {
    var onDrag: ((CGFloat) -> Void)?
    var onDismiss: (() -> Void)?

    private let imageView: UIImageView
    private var fittedSize = CGSize.zero
    private let swipe = SwipeDelegate()

    init(image: UIImage) {
        imageView = UIImageView(image: image)
        super.init(frame: .zero)
        delegate = self
        backgroundColor = .clear
        showsVerticalScrollIndicator = false
        showsHorizontalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        decelerationRate = .fast
        bouncesZoom = true
        addSubview(imageView)

        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(doubleTapped(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(swiped(_:)))
        pan.maximumNumberOfTouches = 1
        swipe.view = self
        pan.delegate = swipe
        addGestureRecognizer(pan)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.size != fittedSize { fit(animated: false) }
        center()
    }

    /// The whole image on screen, as when it opened.
    func fit(animated: Bool) {
        guard bounds.width > 0, bounds.height > 0, let image = imageView.image, image.size.width > 0, image.size.height > 0 else { return }
        if animated, zoomScale != 1 {
            setZoomScale(1, animated: true)
            return
        }
        fittedSize = bounds.size
        let scale = min(bounds.width / image.size.width, bounds.height / image.size.height)
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
        minimumZoomScale = 1
        zoomScale = 1
        imageView.transform = .identity
        imageView.frame = CGRect(origin: .zero, size: size)
        contentSize = size
        // Up to twice the image's own pixels, and at least 4×.
        let pixels = image.size.width * image.scale / (size.width * traitCollection.displayScale)
        maximumZoomScale = max(4, pixels * 2)
        center()
    }

    func step(_ factor: CGFloat) {
        let scale = min(maximumZoomScale, max(minimumZoomScale, zoomScale * factor))
        zoom(around: convert(CGPoint(x: bounds.midX, y: bounds.midY), to: imageView), scale: scale)
    }

    var isZoomedIn: Bool { zoomScale > minimumZoomScale + 0.01 }

    private func zoom(around point: CGPoint, scale: CGFloat) {
        let size = CGSize(width: bounds.width / scale, height: bounds.height / scale)
        zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: true)
    }

    // A smaller image stays centred.
    private func center() {
        let x = max(0, (bounds.width - contentSize.width) / 2)
        let y = max(0, (bounds.height - contentSize.height) / 2)
        let inset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
        if contentInset != inset { contentInset = inset }
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }

    func scrollViewDidZoom(_ scrollView: UIScrollView) { center() }

    @objc private func doubleTapped(_ gesture: UITapGestureRecognizer) {
        if isZoomedIn {
            setZoomScale(minimumZoomScale, animated: true)
        } else {
            zoom(around: gesture.location(in: imageView), scale: min(maximumZoomScale, 3))
        }
    }

    @objc private func swiped(_ pan: UIPanGestureRecognizer) {
        let moved = pan.translation(in: self)
        let progress = min(1, max(0, moved.y / 400))
        switch pan.state {
        case .changed:
            let shrink = 1 - progress * 0.25
            imageView.transform = CGAffineTransform(translationX: moved.x, y: max(0, moved.y)).scaledBy(x: shrink, y: shrink)
            onDrag?(progress)
        case .ended where moved.y > 120 || pan.velocity(in: self).y > 900:
            onDismiss?()
        case .ended, .cancelled, .failed:
            UIView.animate(withDuration: 0.25, delay: 0, usingSpringWithDamping: 0.85, initialSpringVelocity: 0) {
                self.imageView.transform = .identity
            }
            onDrag?(0)
        default:
            break
        }
    }
}

/// Lets the close swipe start only when the image is not zoomed in and the
/// finger moves down, alongside the scroll view's own pan.
private final class SwipeDelegate: NSObject, UIGestureRecognizerDelegate {
    weak var view: ZoomingScrollView?

    func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard let view, let pan = gesture as? UIPanGestureRecognizer, !view.isZoomedIn else { return false }
        let velocity = pan.velocity(in: view)
        return velocity.y > abs(velocity.x)
    }

    func gestureRecognizer(_ gesture: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        true
    }
}
