import Foundation

/// What the last background check saw: requests already notified and
/// top-level agents that were running.
public struct Ledger: Codable, Hashable, Sendable {
    public var requests: Set<String>
    public var running: Set<String>

    public init(requests: Set<String> = [], running: Set<String> = []) {
        self.requests = requests
        self.running = running
    }

    /// What the app showed while it was open, so the background check does not repeat it.
    public init(seen state: DaemonState) {
        requests = Set(state.pending.map(\.id))
        running = Set(state.agents.values.filter { $0.isTopLevel && $0.live && $0.status == .running }.map(\.id))
    }
}

/// Compares a snapshot fetched in the background with the previous check.
/// The snapshot has the pending requests of every workspace, but agents only
/// of the daemon's last used workspace.
public struct BackgroundCheck: Sendable {
    /// Requests that were not notified yet.
    public var approvals: [PendingApproval]
    /// Top-level agents that were running at the last check and are not now.
    public var finished: [AgentView]
    public var ledger: Ledger

    public init(snapshot: Snapshot, previous: Ledger) {
        let agents = (snapshot.workspace?.tabs ?? []).flatMap(\.snapshots).filter { $0.parent == nil }
        let running = Set(agents.filter { $0.live && $0.status == .running }.map(\.id))
        finished = agents.filter { previous.running.contains($0.id) && !running.contains($0.id) && $0.status != .idle }
        approvals = snapshot.pending.filter { !previous.requests.contains($0.id) }
        // Only open requests are remembered, so the ledger stays small.
        ledger = Ledger(requests: Set(snapshot.pending.map(\.id)), running: running)
    }
}
