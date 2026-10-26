import XCTest
@testable import MalacKit

final class PairingTests: XCTestCase {
    func testLinksWithACode() throws {
        let lan = try XCTUnwrap(PairingLink.parse("http://192.168.1.20:7700/#pair=abcde-fghjk"))
        XCTAssertEqual(lan.base.absoluteString, "http://192.168.1.20:7700/")
        XCTAssertEqual(lan.code, "ABCDE-FGHJK")
        XCTAssertNil(lan.token)

        let tunnel = try XCTUnwrap(PairingLink.parse(" https://pig.trycloudflare.com/#pair=ABCDEFGHJK\n"))
        XCTAssertEqual(tunnel.base.absoluteString, "https://pig.trycloudflare.com/")
        XCTAssertEqual(tunnel.code, "ABCDE-FGHJK")

        let proxied = try XCTUnwrap(PairingLink.parse("https://example.com/malac#pair=ABCDE-FGHJK"))
        XCTAssertEqual(proxied.base.absoluteString, "https://example.com/malac/", "a reverse proxy's path is kept")
    }

    func testLinksWithAToken() throws {
        let local = try XCTUnwrap(PairingLink.parse("http://127.0.0.1:7700/?token=8995bbb2"))
        XCTAssertEqual(local.token, "8995bbb2")
        XCTAssertNil(local.code)
        XCTAssertEqual(local.base.absoluteString, "http://127.0.0.1:7700/")
    }

    func testBareAddresses() throws {
        XCTAssertEqual(PairingLink.parse("192.168.1.20:7700")?.base.absoluteString, "http://192.168.1.20:7700/")
        XCTAssertNil(PairingLink.parse("ftp://example.com"))
        XCTAssertNil(PairingLink.parse(""))
    }

    func testCodesNormalizeLikeTheDaemon() {
        XCTAssertEqual(PairingCode.normalize(" abcde-fghjk "), "ABCDEFGHJK")
        XCTAssertEqual(PairingCode.normalize("O1IL0"), "01110")
        XCTAssertTrue(PairingCode.isValid("abcde fghjk"))
        XCTAssertFalse(PairingCode.isValid("ABCDE-FGHJ"), "too short")
        XCTAssertFalse(PairingCode.isValid("ABCDE-FGHJU"), "U is not in the alphabet")
        XCTAssertEqual(PairingCode.format("abcdefghjk"), "ABCDE-FGHJK")
    }

    func testEndpointAddresses() throws {
        let endpoint = DaemonEndpoint(base: URL(string: "https://example.com/malac/")!, token: "t")
        XCTAssertEqual(endpoint.webSocketURL.absoluteString, "wss://example.com/malac/api/ws")
        XCTAssertEqual(endpoint.url("api/sessions/abc").absoluteString, "https://example.com/malac/api/sessions/abc")
        let request = endpoint.request(endpoint.url("api/snapshot"))
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer t")
        let local = DaemonEndpoint(base: URL(string: "http://192.168.1.20:7700/")!, token: "t")
        XCTAssertEqual(local.webSocketURL.absoluteString, "ws://192.168.1.20:7700/api/ws")
    }

    func testFormat() {
        XCTAssertEqual(Format.tokens(950), "950")
        XCTAssertEqual(Format.tokens(7000), "7k")
        XCTAssertEqual(Format.tokens(7240), "7.2k")
        XCTAssertEqual(Format.tokens(1_400_000), "1.4M")
        XCTAssertEqual(Format.cost(0), "")
        XCTAssertEqual(Format.cost(0.0045699), "$0.0046")
        XCTAssertEqual(Format.cost(1.2), "$1.20")
        XCTAssertEqual(Format.duration(milliseconds: 640), "640ms")
        XCTAssertEqual(Format.duration(milliseconds: 1537), "1.5s")
        XCTAssertEqual(Format.duration(milliseconds: 184_000), "3m 04s")
        XCTAssertEqual(Format.duration(milliseconds: 7_500_000), "2h 05m")
        XCTAssertEqual(Format.displayPath("/Users/dev/src/demo", home: "/Users/dev/"), "~/src/demo")
        XCTAssertEqual(Format.displayPath("/Users/devops", home: "/Users/dev"), "/Users/devops")
        XCTAssertEqual(Format.usage(Usage(inputTokens: 27926, outputTokens: 762)), "27.9k in · 762 out")
    }
}
