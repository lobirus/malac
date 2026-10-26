import XCTest
@testable import MalacKit

final class DaemonStateTests: XCTestCase {
    func testReplayEndsWhereTheDaemonsSnapshotDoes() throws {
        let messages = try Fixtures.received()
        let lastSnapshot = try XCTUnwrap(messages.lastIndex { if case .snapshot = $0 { return true }; return false })
        guard case .snapshot(let truth) = messages[lastSnapshot] else { return XCTFail() }
        let (state, _) = Fixtures.replay(Array(messages[..<lastSnapshot]))

        let views = try XCTUnwrap(truth.workspace?.tabs.first?.snapshots)
        XCTAssertEqual(views.count, 3)
        for view in views {
            let agent = try XCTUnwrap(state.agents[view.id], view.label)
            XCTAssertEqual(agent.status, view.status, view.label)
            XCTAssertEqual(agent.usage, view.usage, view.label)
            XCTAssertEqual(agent.parent, view.parent, view.label)
            XCTAssertEqual(agent.model, view.model, view.label)
            XCTAssertEqual(agent.activity, view.activity, view.label)
            XCTAssertEqual(agent.report ?? "", view.report ?? "", view.label)
        }
        let tab = try XCTUnwrap(state.workspace?.tabs.first)
        XCTAssertEqual(tab.name, "demo app")
        XCTAssertEqual(tab.mode, .edits)
        XCTAssertEqual(tab.agents, [Fixtures.main, Fixtures.second])
        XCTAssertEqual(state.jobs["19959044"]?.status, .killed)
        XCTAssertTrue(state.pending.isEmpty)
        XCTAssertEqual(state.phase(of: tab), .done, "one agent completed, the other was stopped")
    }

    func testApprovalsComeAndGo() throws {
        var state = DaemonState()
        var requested: [String] = []
        var resolved: [String] = []
        var openAfterRequest: [Int] = []
        for message in try Fixtures.received() {
            for effect in state.apply(message) {
                switch effect {
                case .approvalRequested(let approval):
                    requested.append(approval.id)
                    openAfterRequest.append(state.pending.count)
                    XCTAssertEqual(state.place(of: approval), "demo · default")
                case .approvalResolved(let id):
                    resolved.append(id)
                default: break
                }
            }
        }
        XCTAssertEqual(requested.count, 4)
        XCTAssertEqual(resolved, requested, "every request was answered, by this client or another")
        XCTAssertEqual(openAfterRequest, [1, 1, 1, 1])
        XCTAssertTrue(state.pending.isEmpty)
    }

    func testAnotherClientsSnapshotShowsTheOpenRequest() throws {
        let snapshot = try Fixtures.decode(Snapshot.self, "snapshot-pending.json")
        var state = DaemonState()
        state.apply(.snapshot(snapshot))
        let main = try XCTUnwrap(state.agents[Fixtures.main])
        XCTAssertEqual(state.phase(of: main), .awaitingApproval)
        XCTAssertEqual(main.activity, "waiting for approval: bash")
        XCTAssertEqual(state.pending(for: Fixtures.main).first?.summary, "cat main.rs")
        XCTAssertEqual(state.phase(of: state.workspaces[0]), .awaitingApproval)

        let effects = state.apply(.event(tab: Fixtures.tab, workspace: Fixtures.workspace, envelope: Envelope(
            agent: Fixtures.main, seq: 39, at: Date(),
            event: .permissionResolved(requestId: "01a176fa-9fa1-7585-b237-c13f0d7a3e3f", allowed: true)
        )))
        XCTAssertEqual(effects, [.approvalResolved(requestId: "01a176fa-9fa1-7585-b237-c13f0d7a3e3f")])
        XCTAssertTrue(state.pending.isEmpty)
        XCTAssertEqual(state.phase(of: state.agents[Fixtures.main]!), .running)
    }

    func testOnlyTopLevelTurnsAreReported() throws {
        let (_, effects) = Fixtures.replay(try Fixtures.received())
        let turns = effects.compactMap { effect -> (String, TurnStatus)? in
            if case let .turnFinished(agent, status, _) = effect { return (agent.id, status) }
            return nil
        }
        XCTAssertEqual(turns.map(\.0), [Fixtures.main, Fixtures.main, Fixtures.second])
        XCTAssertEqual(turns.map(\.1), [.completed, .completed, .cancelled])
    }

    func testATabWithoutAWorkspaceAsksForASnapshot() throws {
        let messages = try Fixtures.received()
        var state = DaemonState()
        state.apply(messages[0])
        XCTAssertNil(state.workspace)
        let effects = messages[1 ... 3].flatMap { state.apply($0) }
        XCTAssertEqual(effects, [.resync(workspace: nil)], "tab_create made a default workspace this connection has not seen")
        XCTAssertEqual(state.workspaces.first?.name, "default")
    }

    func testLaggedAndDeletedWorkspacesResync() throws {
        var state = DaemonState()
        state.apply(.snapshot(try Fixtures.decode(Snapshot.self, "rest/snapshot.json")))
        XCTAssertEqual(state.apply(.lagged(skipped: 3)), [.resync(workspace: Fixtures.workspace)])
        XCTAssertEqual(state.apply(.workspaceDeleted(Fixtures.workspace)), [.resync(workspace: nil)])
        XCTAssertNil(state.workspace)
        XCTAssertTrue(state.workspaces.isEmpty)
    }

    func testSnapshotsResetTheLogs() throws {
        let messages = try Fixtures.received()
        var state = DaemonState()
        for message in messages.prefix(60) { state.apply(message) }
        let log = state.log(Fixtures.main)
        XCTAssertFalse(log.isEmpty)
        let deltas = log.filter { if case .event(let e) = $0, case .thinkingDelta = e.event { return true }; return false }
        XCTAssertEqual(deltas.count, 1, "33 thinking deltas in a row are merged into one entry")
        let generation = state.generation
        state.apply(messages[0])
        XCTAssertTrue(state.log(Fixtures.main).isEmpty)
        XCTAssertEqual(state.generation, generation + 1)
    }

    func testModeForEveryTabOfAnotherWorkspaceIsIgnored() throws {
        var state = DaemonState()
        state.apply(.snapshot(try Fixtures.decode(Snapshot.self, "rest/snapshot.json")))
        state.apply(.mode(.autopilot, tab: nil, workspace: "elsewhere"))
        XCTAssertEqual(state.workspace?.tabs.first?.mode, .edits)
        state.apply(.mode(.autopilot, tab: nil, workspace: Fixtures.workspace))
        XCTAssertEqual(state.workspace?.tabs.first?.mode, .autopilot)
    }

    func testBackgroundCheckNotifiesOnce() throws {
        let pending = try Fixtures.decode(Snapshot.self, "snapshot-pending.json")
        let first = BackgroundCheck(snapshot: pending, previous: Ledger())
        XCTAssertEqual(first.approvals.map(\.summary), ["cat main.rs"])
        XCTAssertEqual(first.ledger.running, [Fixtures.main])
        let again = BackgroundCheck(snapshot: pending, previous: first.ledger)
        XCTAssertTrue(again.approvals.isEmpty)

        let done = try Fixtures.decode(Snapshot.self, "rest/snapshot.json")
        let later = BackgroundCheck(snapshot: done, previous: again.ledger)
        XCTAssertEqual(later.finished.map(\.id), [Fixtures.main])
        XCTAssertTrue(later.ledger.requests.isEmpty)
    }
}
