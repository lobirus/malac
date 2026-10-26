import XCTest
@testable import MalacKit

final class ProtocolTests: XCTestCase {
    func testEveryRecordedMessageDecodesToAKnownType() throws {
        let messages = try Fixtures.received()
        XCTAssertEqual(messages.count, 391)
        for message in messages {
            if case .unknown(let type, _) = message { XCTFail("unknown message type \(type)") }
            if case .event(_, _, let envelope) = message, case .unknown(let type) = envelope.event {
                XCTFail("unknown event type \(type)")
            }
        }
        guard case .snapshot(let first) = messages.first else { return XCTFail("the first message is a snapshot") }
        XCTAssertEqual(first.daemon.version, "1.0.1")
        XCTAssertEqual(first.daemon.userHome, "/Users/dev")
        XCTAssertNil(first.workspace)
        XCTAssertEqual(first.defaultModel, "lora/fast")
        XCTAssertTrue(first.models.contains("lora/smart"))
        guard case .shutdown(let reason) = messages.last else { return XCTFail("the last message is shutdown") }
        XCTAssertEqual(reason, "stopped by a client")
    }

    func testRecordedEventsCarryTheirFields() throws {
        let envelopes = try Fixtures.received().compactMap { message -> Envelope? in
            if case .event(_, _, let envelope) = message { return envelope }
            return nil
        }
        XCTAssertEqual(envelopes.map(\.seq), envelopes.map(\.seq).sorted(), "seq increases")
        let request = envelopes.lazy.compactMap { envelope -> (String, String, JSONValue)? in
            if case let .permissionRequested(_, callId, tool, _, input) = envelope.event { return (callId, tool, input) }
            return nil
        }.first
        XCTAssertEqual(request?.0, "call_01a176fa9f6471e5b9c94673")
        XCTAssertEqual(request?.1, "bash")
        XCTAssertEqual(request?.2["command"]?.stringValue, "cat main.rs")

        let spawned = envelopes.first { $0.agent == Fixtures.reader }
        guard case let .agentStarted(label, parent, model, cwd)? = spawned?.event else { return XCTFail("reader starts") }
        XCTAssertEqual(label, "reader")
        XCTAssertEqual(parent, Fixtures.main)
        XCTAssertEqual(model, "lora/fast")
        XCTAssertEqual(cwd, "/Users/dev/src/demo")

        let cancelled = envelopes.last { envelope in
            if case .turnFinished = envelope.event { return envelope.agent == Fixtures.second }
            return false
        }
        guard case let .turnFinished(status, _)? = cancelled?.event else { return XCTFail("the second agent's turn ends") }
        XCTAssertEqual(status, .cancelled)
    }

    func testDocSamplesDecode() throws {
        let samples = try Fixtures.docSamples()
        guard case .snapshot(let snapshot) = samples[0] else { return XCTFail("snapshot") }
        let tab = try XCTUnwrap(snapshot.workspace?.tabs.first)
        XCTAssertEqual(tab.options?.model, "lora/smart")
        XCTAssertEqual(tab.mode, .ask)
        XCTAssertEqual(tab.snapshots.first?.usage, Usage(inputTokens: 7012, outputTokens: 20))
        XCTAssertEqual(tab.snapshots.last?.title, "add pagination")
        XCTAssertEqual(snapshot.pending.first?.callId, "call_1")

        guard case .event(_, _, let delta) = samples[1], case .textDelta(let text) = delta.event else { return XCTFail("text_delta") }
        XCTAssertEqual(text, "Done")
        XCTAssertEqual(delta.at, Timestamp.parse("2026-10-25T18:34:00.500Z"))
        guard case .event(nil, nil, let update) = samples[2], case .update(let status) = update.event else { return XCTFail("update") }
        XCTAssertTrue(status.available)
        guard case .ack(let ack) = samples[3] else { return XCTFail("ack") }
        XCTAssertEqual(ack.attachments.first?.kind, "text")
        XCTAssertEqual(ack.clientRef, "c7")
        guard case .launched(_, _, _, nil) = samples[4] else { return XCTFail("launched") }
        guard case .error("c8", _) = samples[5] else { return XCTFail("error") }
        guard case .mode(.autopilot, nil, _) = samples[10] else { return XCTFail("mode") }
        guard case .lagged(12) = samples[14] else { return XCTFail("lagged") }
        guard case .shutdown("idle") = samples[15] else { return XCTFail("shutdown") }
        guard case .unknown("pairing_requested", nil) = samples[16] else { return XCTFail("an unknown type is kept, not an error") }
    }

    func testRestFixturesDecode() throws {
        struct Sessions: Decodable { var sessions: [SessionSummary] }
        let sessions = try Fixtures.decode(Sessions.self, "rest/sessions.json").sessions
        XCTAssertEqual(sessions.map(\.label), ["main", "reader"])
        XCTAssertNotNil(sessions[0].updated, "nanosecond timestamps parse")

        let transcript = try Fixtures.decode(SessionTranscript.self, "rest/session-main.json")
        XCTAssertEqual(transcript.history.count, 10)
        guard case .toolResult(let id, let content, false) = transcript.history[2].content.first else { return XCTFail("tool_result") }
        XCTAssertEqual(id, "call_01a176fa9f6471e5b9c94673")
        XCTAssertEqual(Block.resultText(content), "fn main() {\n    println!(\"hello\");\n}")

        let output = try Fixtures.decode(JobOutput.self, "rest/job-output.json")
        XCTAssertEqual(output.text, "tick 1\ntick 2")
        XCTAssertEqual(output.job.status, .running)

        struct Jobs: Decodable { var jobs: [JobView] }
        let job = try XCTUnwrap(Fixtures.decode(Jobs.self, "rest/jobs.json").jobs.first)
        XCTAssertEqual(job.status, .killed)
        XCTAssertNil(job.exitCode)
        XCTAssertEqual(job.tab, Fixtures.tab)

        let snapshot = try Fixtures.decode(Snapshot.self, "rest/snapshot.json")
        XCTAssertEqual(snapshot.workspace?.tabs.first?.name, "demo app")
        let daemon = try Fixtures.decode(DaemonStatus.self, "rest/daemon.json")
        XCTAssertEqual(daemon.pid, 81049)
    }

    func testTimestamps() {
        let base = Timestamp.parse("2026-10-26T06:51:44Z")
        XCTAssertNotNil(base)
        XCTAssertEqual(Timestamp.parse("2026-10-26T06:51:44.434082473Z")!.timeIntervalSince(base!), 0.434082473, accuracy: 1e-6)
        XCTAssertEqual(Timestamp.parse("2026-10-26T08:51:44+02:00"), base)
        XCTAssertEqual(Timestamp.parse("2026-10-26T06:51:44.5-00:30")!.timeIntervalSince(base!), 1800.5, accuracy: 1e-6)
        XCTAssertNil(Timestamp.parse("…"))
        XCTAssertNil(Timestamp.parse("2026-10-26T06:51:44"))
    }

    func testUnknownEnumValuesFallBack() throws {
        let json = #"{"id":"a","label":"main","status":"hibernating","usage":{"input_tokens":1,"output_tokens":2}}"#
        let agent = try JSONDecoder.malac().decode(AgentView.self, from: Data(json.utf8))
        XCTAssertEqual(agent.status, .idle)
        XCTAssertEqual(agent.usage.cacheReadTokens, 0)
        XCTAssertEqual(agent.cwd, "")
    }

    func testCommandsEncodeLikeTheProtocol() throws {
        func json(_ command: Command, ref: String? = nil) throws -> [String: Any] {
            try JSONSerialization.jsonObject(with: command.encoded(clientRef: ref)) as! [String: Any]
        }
        let prompt = try json(.prompt(text: "hi", agent: nil, tab: "t1", images: [ImageInput(mediaType: "image/png", data: "AAAA", name: "shot.png")], model: "lora/smart"), ref: "m1")
        XCTAssertEqual(prompt["type"] as? String, "prompt")
        XCTAssertEqual(prompt["client_ref"] as? String, "m1")
        XCTAssertEqual(prompt["tab"] as? String, "t1")
        XCTAssertNil(prompt["agent"])
        XCTAssertEqual(prompt["model"] as? String, "lora/smart")
        let image = (prompt["images"] as? [[String: String]])?.first
        XCTAssertEqual(image, ["media_type": "image/png", "data": "AAAA", "name": "shot.png"])

        let plain = try json(.prompt(text: "again", agent: "a1", tab: nil))
        XCTAssertNil(plain["images"], "no empty arrays on the wire")
        XCTAssertNil(plain["client_ref"])

        let approve = try json(.approve(requestId: "r1", decision: .allowSession))
        XCTAssertEqual(approve as NSDictionary, ["type": "approve", "request_id": "r1", "decision": "allow_session"])
        XCTAssertEqual(try json(.snapshot(workspace: "w1")) as NSDictionary, ["type": "snapshot", "workspace": "w1"])
        XCTAssertEqual(try json(.mode(.autopilot, tab: "t1")) as NSDictionary, ["type": "mode", "mode": "autopilot", "tab": "t1"])
        XCTAssertEqual(try json(.jobOutput(job: "19959044", tail: 400)) as NSDictionary, ["type": "job_output", "job": "19959044", "tail": 400])
    }

    func testRecordedCommandsMatchTheEncoder() throws {
        let sent = try Fixtures.session().filter(\.outgoing).map { try JSONSerialization.jsonObject(with: $0.json) as! NSDictionary }
        let approve = try XCTUnwrap(sent.first { $0["type"] as? String == "approve" })
        let encoded = try JSONSerialization.jsonObject(with: Command.approve(
            requestId: approve["request_id"] as! String, decision: .allow
        ).encoded()) as! NSDictionary
        XCTAssertEqual(encoded, approve)
    }
}
