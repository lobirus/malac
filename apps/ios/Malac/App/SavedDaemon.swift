import Foundation
import MalacKit

/// A daemon this phone is paired with. The device token lives in the Keychain.
struct SavedDaemon: Identifiable, Codable, Hashable {
    var id = UUID()
    var name: String
    var base: URL
    /// The daemon's id for this device, to revoke it when forgetting the daemon.
    var deviceId: String?
    var added = Date()

    var host: String {
        let host = base.host() ?? base.absoluteString
        return base.port.map { "\(host):\($0)" } ?? host
    }
}

/// The saved daemons, which one is selected, and what the last background
/// check saw for each. Tokens are not here; see `Keychain`.
struct DaemonStore {
    private let defaults: UserDefaults
    private let keychain: Keychain

    init(defaults: UserDefaults = .standard, keychain: Keychain = Keychain()) {
        self.defaults = defaults
        self.keychain = keychain
    }

    var daemons: [SavedDaemon] {
        get {
            guard let data = defaults.data(forKey: "daemons") else { return [] }
            return (try? JSONDecoder().decode([SavedDaemon].self, from: data)) ?? []
        }
        nonmutating set {
            defaults.set(try? JSONEncoder().encode(newValue), forKey: "daemons")
        }
    }

    var selected: UUID? {
        get { defaults.string(forKey: "selectedDaemon").flatMap(UUID.init(uuidString:)) }
        nonmutating set { defaults.set(newValue?.uuidString, forKey: "selectedDaemon") }
    }

    func endpoint(_ daemon: SavedDaemon) -> DaemonEndpoint? {
        keychain.token(for: daemon.id).map { DaemonEndpoint(base: daemon.base, token: $0) }
    }

    func saveToken(_ token: String, for daemon: SavedDaemon) throws {
        try keychain.set(token, for: daemon.id)
    }

    func removeToken(for daemon: SavedDaemon) {
        keychain.remove(for: daemon.id)
        defaults.removeObject(forKey: ledgerKey(daemon))
    }

    func ledger(_ daemon: SavedDaemon) -> Ledger {
        guard let data = defaults.data(forKey: ledgerKey(daemon)) else { return Ledger() }
        return (try? JSONDecoder().decode(Ledger.self, from: data)) ?? Ledger()
    }

    func setLedger(_ ledger: Ledger, for daemon: SavedDaemon) {
        defaults.set(try? JSONEncoder().encode(ledger), forKey: ledgerKey(daemon))
    }

    private func ledgerKey(_ daemon: SavedDaemon) -> String { "ledger.\(daemon.id.uuidString)" }
}
