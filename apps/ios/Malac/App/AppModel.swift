import BackgroundTasks
import MalacKit
import Observation
import UIKit

/// The paired daemons and the session with the selected one. Keeps the
/// WebSocket open while the app is in the foreground and for a short grace
/// period after; background refresh checks every daemon from then on.
@MainActor
@Observable
final class AppModel {
    enum Tab: Hashable { case workspaces, approvals, jobs, daemons }

    /// Something a notification asked to show.
    struct Destination: Equatable {
        var agent: String
    }

    private(set) var daemons: [SavedDaemon]
    private(set) var session: DaemonSession?
    var tab = Tab.workspaces
    var destination: Destination?
    /// Set when the selected daemon refused its token, to offer pairing again.
    var repairing: SavedDaemon?

    @ObservationIgnored let notifier = Notifier()
    @ObservationIgnored let pairing: PairingService = DaemonPairing()
    @ObservationIgnored let push: PushRegistrar = NoPush()
    @ObservationIgnored private let store = DaemonStore()
    @ObservationIgnored private let network = NetworkMonitor()
    @ObservationIgnored private var inForeground = true
    @ObservationIgnored private var background: (task: UIBackgroundTaskIdentifier, stop: Task<Void, Never>)?

    static let refreshTask = "io.malac.app.refresh"
    /// How long the socket stays open after the app leaves the screen.
    private static let grace: Duration = .seconds(25)

    init() {
        daemons = store.daemons
        notifier.registerCategories()
        select(daemons.first { $0.id == store.selected } ?? daemons.first)
        network.start { [weak self] in self?.session?.reconnectNow() }
    }

    var selected: SavedDaemon? { session?.daemon }

    // MARK: Daemons

    func select(_ daemon: SavedDaemon?) {
        guard daemon?.id != session?.daemon.id || session == nil else { return }
        session?.stop()
        session = nil
        store.selected = daemon?.id
        guard let daemon, let endpoint = store.endpoint(daemon) else { return }
        let session = DaemonSession(daemon: daemon, endpoint: endpoint, notifier: notifier)
        session.inForeground = inForeground
        self.session = session
        if inForeground { session.start() }
    }

    /// Saves a daemon this phone just paired with, or a new token for one it knew.
    func add(base: URL, name: String, token: String, deviceId: String?) throws {
        var daemon = daemons.first { $0.base == base } ?? SavedDaemon(name: name, base: base)
        daemon.name = name
        daemon.deviceId = deviceId
        try store.saveToken(token, for: daemon)
        if let index = daemons.firstIndex(where: { $0.id == daemon.id }) {
            daemons[index] = daemon
        } else {
            daemons.append(daemon)
        }
        store.daemons = daemons
        if session?.daemon.id == daemon.id { session?.stop(); session = nil }
        select(daemon)
        repairing = nil
        Task { await notifier.requestAuthorization() }
    }

    func rename(_ daemon: SavedDaemon, to name: String) {
        guard let index = daemons.firstIndex(where: { $0.id == daemon.id }), !name.isEmpty else { return }
        daemons[index].name = name
        store.daemons = daemons
    }

    /// Forgets a daemon, revoking this device's token first when the daemon answers.
    func forget(_ daemon: SavedDaemon) async {
        if let endpoint = store.endpoint(daemon) {
            let api = DaemonAPI(endpoint: endpoint)
            var device = daemon.deviceId
            if device == nil { device = try? await api.devices().first { $0.current }?.id }
            if let device { try? await api.revoke(device: device) }
        }
        store.removeToken(for: daemon)
        daemons.removeAll { $0.id == daemon.id }
        store.daemons = daemons
        if session?.daemon.id == daemon.id { select(daemons.first) }
    }

    // MARK: Foreground and background

    func enterForeground() {
        inForeground = true
        if let background {
            background.stop.cancel()
            UIApplication.shared.endBackgroundTask(background.task)
            self.background = nil
        }
        session?.inForeground = true
        session?.start()
    }

    func enterBackground() {
        inForeground = false
        session?.inForeground = false
        if let session { store.setLedger(session.ledger, for: session.daemon) }
        scheduleRefresh()
        guard background == nil else { return }
        let task = UIApplication.shared.beginBackgroundTask(withName: "malac connection") { [weak self] in
            Task { @MainActor in self?.closeAfterGrace() }
        }
        let stop = Task { [weak self] in
            try? await Task.sleep(for: Self.grace)
            guard !Task.isCancelled else { return }
            self?.closeAfterGrace()
        }
        background = (task, stop)
    }

    private func closeAfterGrace() {
        guard let background, !inForeground else { return }
        if let session {
            store.setLedger(session.ledger, for: session.daemon)
            session.stop()
        }
        UIApplication.shared.endBackgroundTask(background.task)
        self.background = nil
    }

    func scheduleRefresh() {
        let request = BGAppRefreshTaskRequest(identifier: Self.refreshTask)
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        try? BGTaskScheduler.shared.submit(request)
    }

    /// Background refresh: asks every daemon for its snapshot and notifies
    /// about requests and finished turns the user has not seen.
    func checkInBackground() async {
        scheduleRefresh()
        for daemon in daemons {
            guard let endpoint = store.endpoint(daemon), let snapshot = try? await DaemonAPI(endpoint: endpoint).snapshot() else { continue }
            var state = DaemonState()
            state.apply(.snapshot(snapshot))
            let check = BackgroundCheck(snapshot: snapshot, previous: store.ledger(daemon))
            for request in check.approvals {
                notifier.approval(request, place: state.place(of: request), daemon: daemon)
            }
            for agent in check.finished {
                let name = agent.title.isEmpty ? agent.label : agent.title
                notifier.turnFinished(agent: agent.id, name: name, status: Self.turnStatus(agent.status), text: agent.report ?? "", daemon: daemon)
            }
            store.setLedger(check.ledger, for: daemon)
        }
    }

    private static func turnStatus(_ status: AgentStatus) -> TurnStatus {
        switch status {
        case .failed: .failed
        case .cancelled: .cancelled
        default: .completed
        }
    }

    // MARK: Notifications

    /// An approval answered from a notification, possibly with the app in the background.
    func answer(_ target: Notifier.Target, _ action: Notifier.Action) async {
        guard let request = target.request,
              let daemon = daemons.first(where: { $0.id == target.daemon }),
              let endpoint = store.endpoint(daemon) else { return }
        _ = try? await DaemonAPI(endpoint: endpoint).command(.approve(requestId: request, decision: action.decision))
    }

    func open(_ target: Notifier.Target) {
        if let daemon = daemons.first(where: { $0.id == target.daemon }) { select(daemon) }
        if let agent = target.agent {
            tab = .workspaces
            destination = Destination(agent: agent)
        } else if target.request != nil {
            tab = .approvals
        }
    }

    func register(deviceToken: Data) async {
        for daemon in daemons { await push.register(deviceToken: deviceToken, for: daemon) }
    }
}
