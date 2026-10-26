import MalacKit
import UIKit

/// A photo or screenshot for a prompt, downscaled and sent as base64 JPEG.
struct ImageAttachment: Identifiable {
    let id = UUID()
    let preview: UIImage
    let input: ImageInput

    private static let longestSide: CGFloat = 2048

    init?(data: Data, name: String) {
        guard let image = UIImage(data: data) else { return nil }
        let scale = min(1, Self.longestSide / max(image.size.width, image.size.height))
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        guard let jpeg = resized.jpegData(compressionQuality: 0.85) else { return nil }
        preview = resized
        input = ImageInput(mediaType: "image/jpeg", data: jpeg.base64EncodedString(), name: name)
    }
}
