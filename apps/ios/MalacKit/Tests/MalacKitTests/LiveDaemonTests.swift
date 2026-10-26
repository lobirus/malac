import XCTest
@testable import MalacKit

/// Pairs with a running daemon and drives a connection through it. Skipped
/// unless `MALAC_LIVE_URL` and `MALAC_LIVE_CODE` (from `malac remote pair`)
/// are set; it revokes the device it paired at the end.
@MainActor
final class LiveDaemonTests: XCTestCase {
    func testPairConnectAndRevoke() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let address = environment["MALAC_LIVE_URL"], let code = environment["MALAC_LIVE_CODE"] else {
            throw XCTSkip("set MALAC_LIVE_URL and MALAC_LIVE_CODE to run against a daemon")
        }
        let link = try XCTUnwrap(PairingLink.parse(address))
        let pairing = DaemonPairing()
        let hello = try await pairing.hello(link.base)
        XCTAssertEqual(hello.app, "malac")
        let paired = try await pairing.pair(link.base, code: code, deviceName: "MalacKit tests")
        let endpoint = DaemonEndpoint(base: link.base, token: paired.token)

        let connection = DaemonConnection(endpoint: endpoint)
        let online = expectation(description: "snapshot")
        let refused = expectation(description: "unauthorized after revoking")
        var snapshot: Snapshot?
        connection.onMessage = { message in
            if case .snapshot(let value) = message, snapshot == nil {
                snapshot = value
                online.fulfill()
            }
        }
        connection.onStatus = { status in
            if status == .unauthorized { refused.fulfill() }
        }
        connection.start()
        await fulfillment(of: [online], timeout: 10)
        XCTAssertEqual(snapshot?.daemon.version, hello.version)

        guard case .pong = try await connection.request(.ping) else { return XCTFail("pong") }
        do {
            _ = try await connection.request(.agentOpen(agent: "no-such-agent", tab: nil))
            XCTFail("an unknown agent is an error reply")
        } catch DaemonError.command {}

        let api = DaemonAPI(endpoint: endpoint)
        let devices = try await api.devices()
        let me = try XCTUnwrap(devices.first { $0.current })
        XCTAssertEqual(me.name, "MalacKit tests")
        _ = try await api.snapshot()
        try await api.revoke(device: me.id)
        await fulfillment(of: [refused], timeout: 20)
        connection.stop()
    }
}
