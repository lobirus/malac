import AVFoundation
import SwiftUI

/// A full-screen camera that reports the first QR code it sees.
struct QRScannerSheet: View {
    let found: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var access = AVCaptureDevice.authorizationStatus(for: .video)

    var body: some View {
        NavigationStack {
            Group {
                switch access {
                case .authorized:
                    QRScanner(found: found)
                        .ignoresSafeArea()
                        .overlay {
                            RoundedRectangle(cornerRadius: 12)
                                .strokeBorder(.white.opacity(0.8), lineWidth: 2)
                                .frame(width: 240, height: 240)
                                .accessibilityHidden(true)
                        }
                case .notDetermined:
                    ProgressView().task {
                        _ = await AVCaptureDevice.requestAccess(for: .video)
                        access = AVCaptureDevice.authorizationStatus(for: .video)
                    }
                default:
                    ContentUnavailableView {
                        Label("No camera access", systemImage: "camera")
                    } description: {
                        Text("Allow malac to use the camera in Settings, or type the address and code instead.")
                    } actions: {
                        Button("Open Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                        }
                    }
                }
            }
            .background(Color.black)
            .navigationTitle("Scan the pairing code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> ScannerController {
        let controller = ScannerController()
        controller.found = found
        return controller
    }

    func updateUIViewController(_ controller: ScannerController, context: Context) {}
}

private final class ScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var found: ((String) -> Void)?
    private let session = AVCaptureSession()
    private let queue = DispatchQueue(label: "io.malac.app.camera")
    private var preview: AVCaptureVideoPreviewLayer?
    private var done = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let camera = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: camera),
              session.canAddInput(input) else {
            let label = UILabel()
            label.text = "This device has no camera."
            label.textColor = .white
            label.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(label)
            NSLayoutConstraint.activate([
                label.centerXAnchor.constraint(equalTo: view.centerXAnchor),
                label.centerYAnchor.constraint(equalTo: view.centerYAnchor),
            ])
            return
        }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)
        self.preview = preview
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        let session = session
        queue.async { session.startRunning() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let session = session
        queue.async { session.stopRunning() }
    }

    // The delegate queue is the main queue.
    nonisolated func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard let code = objects.compactMap({ ($0 as? AVMetadataMachineReadableCodeObject)?.stringValue }).first else { return }
        MainActor.assumeIsolated { scanned(code) }
    }

    private func scanned(_ code: String) {
        guard !done else { return }
        done = true
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        found?(code)
    }
}
