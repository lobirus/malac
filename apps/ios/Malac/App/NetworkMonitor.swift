import Foundation
import Network

/// Calls `onChange` when the phone moves to another network, so an open
/// socket that may have died with the old one is replaced at once.
final class NetworkMonitor {
    private let monitor = NWPathMonitor()
    private let queue = DispatchQueue(label: "io.malac.app.network")
    private var last: String?

    func start(onChange: @escaping @MainActor () -> Void) {
        monitor.pathUpdateHandler = { [weak self] path in
            guard let self else { return }
            let signature = path.status == .satisfied ? path.availableInterfaces.map(\.name).joined(separator: ",") : "offline"
            defer { self.last = signature }
            guard let last = self.last, last != signature, path.status == .satisfied else { return }
            Task { @MainActor in onChange() }
        }
        monitor.start(queue: queue)
    }
}
