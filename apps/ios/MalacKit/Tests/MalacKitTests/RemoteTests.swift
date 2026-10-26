import XCTest
@testable import MalacKit

/// Pairing and remote access, from fixtures recorded on a live daemon (`apps/fixtures/remote`).
final class RemoteTests: XCTestCase {
    func testPairingResponsesDecode() throws {
        let hello = try Fixtures.decode(Hello.self, "remote/hello.json")
        XCTAssertEqual(hello.app, "malac")
        XCTAssertTrue(hello.pairing)

        let paired = try Fixtures.decode(Paired.self, "remote/paired.json")
        XCTAssertEqual(paired.device?.name, "Test iPhone")
        XCTAssertEqual(paired.device?.kind, "app")
        XCTAssertEqual(paired.device?.current, true)

        struct Devices: Decodable { var devices: [DeviceView] }
        let devices = try Fixtures.decode(Devices.self, "remote/devices.json").devices
        XCTAssertEqual(devices.first { $0.current }?.id, paired.device?.id)
    }

    func testFailuresMapToErrors() throws {
        struct Recorded: Decodable {
            var status: Int
            var body: JSONValue
        }
        let invalid = try Fixtures.decode(Recorded.self, "remote/pair-invalid.json")
        let body = try JSONEncoder().encode(invalid.body)
        XCTAssertEqual(HTTP.error(status: invalid.status, body: body), .forbidden("the pairing code is wrong, used or expired"))

        let revoked = try Fixtures.decode(Recorded.self, "remote/revoked.json")
        XCTAssertEqual(HTTP.error(status: revoked.status, body: Data((revoked.body.stringValue ?? "").utf8)), .unauthorized)
    }

    func testPairingLinksFromTheDaemon() throws {
        struct Pairing: Decodable {
            struct Link: Decodable { var url: String }
            var links: [Link]
        }
        let link = try XCTUnwrap(Fixtures.decode(Pairing.self, "remote/pairing-code.json").links.first)
        let parsed = try XCTUnwrap(PairingLink.parse(link.url))
        XCTAssertEqual(parsed.base.absoluteString, "http://127.0.0.1:7791/")
        XCTAssertEqual(parsed.code, "ABCDE-FGHJK")
    }

    func testRemoteAndAwakeMessages() throws {
        let array = try JSONSerialization.jsonObject(with: Fixtures.data("remote/messages.json")) as! [Any]
        let messages = try array.map { try ServerMessage.decode(JSONSerialization.data(withJSONObject: $0)) }
        var state = DaemonState()
        for message in messages { state.apply(message) }
        guard case .snapshot(let snapshot) = messages[0] else { return XCTFail("snapshot") }
        XCTAssertEqual(snapshot.awake?.method, "caffeinate")
        XCTAssertEqual(snapshot.remote?.urls.first?.kind, "local")
        XCTAssertEqual(state.awake?.mode, .busy)
        XCTAssertEqual(state.awake?.awake, true)
        XCTAssertEqual(state.remote?.devices, 1, "the second device paired, then was revoked")
        XCTAssertFalse(state.remote?.enabled ?? true)
        XCTAssertEqual(try JSONSerialization.jsonObject(with: Command.keepAwake(.always).encoded()) as? NSDictionary,
                       ["type": "keep_awake", "mode": "always"])
    }
}
