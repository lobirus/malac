import Foundation
@testable import MalacKit

/// The protocol fixtures shared with the Android tests, in `apps/fixtures`:
/// a session recorded from a live daemon and the samples from `docs/protocol.md`.
enum Fixtures {
    static let directory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // MalacKitTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // MalacKit
        .deletingLastPathComponent() // ios
        .deletingLastPathComponent() // apps
        .appending(path: "fixtures")

    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: directory.appending(path: name))
    }

    static func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        try JSONDecoder.malac().decode(T.self, from: data(name))
    }

    /// The recorded session's lines: what the client sent and what it received.
    static func session() throws -> [(outgoing: Bool, json: Data)] {
        let text = String(decoding: try data("session.jsonl"), as: UTF8.self)
        return try text.split(separator: "\n").map { line in
            let object = try JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
            let message = try JSONSerialization.data(withJSONObject: object["msg"]!)
            return (object["dir"] as? String == "out", message)
        }
    }

    /// Every message the recorded client received, decoded.
    static func received() throws -> [ServerMessage] {
        try session().filter { !$0.outgoing }.map { try ServerMessage.decode($0.json) }
    }

    /// The protocol doc's samples.
    static func docSamples() throws -> [ServerMessage] {
        let array = try JSONSerialization.jsonObject(with: data("protocol-doc.json")) as! [Any]
        return try array.map { try ServerMessage.decode(JSONSerialization.data(withJSONObject: $0)) }
    }

    /// The state after replaying the recorded messages.
    static func replay(_ messages: [ServerMessage]) -> (DaemonState, [Effect]) {
        var state = DaemonState()
        var effects: [Effect] = []
        for message in messages { effects += state.apply(message) }
        return (state, effects)
    }

    static let main = "01a176fa-99d2-7014-9be0-383c2c41ddc3"
    static let reader = "01a176fa-a30f-7178-9dba-d02eeede0201"
    static let second = "01a176fa-c603-748e-85a9-2971e96f0c17"
    static let tab = "01a176fa-99d0-733d-bec9-e9d000301f35"
    static let workspace = "01a176fa-99d0-733d-bec9-e9cf16452a88"
}
