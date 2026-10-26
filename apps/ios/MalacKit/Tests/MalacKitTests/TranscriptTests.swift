import XCTest
@testable import MalacKit

final class TranscriptTests: XCTestCase {
    /// The logs a client builds from the recorded events, without the snapshots that reset them.
    private func liveState() throws -> DaemonState {
        let events = try Fixtures.received().filter { if case .event = $0 { return true }; return false }
        return Fixtures.replay(events).0
    }

    private func tools(_ items: [TranscriptItem]) -> [ToolCall] {
        items.compactMap { if case .tool(let call) = $0 { return call }; return nil }
    }

    func testSavedHistory() throws {
        let saved = try Fixtures.decode(SessionTranscript.self, "rest/session-main.json")
        let transcript = Transcript(history: saved.history, log: [], after: 0, pending: [], children: [])
        let items = transcript.items

        guard case .user(let prompt) = items.first else { return XCTFail("the prompt comes first") }
        XCTAssertTrue(prompt.text.hasPrefix("Do these steps in order"))
        let calls = tools(items)
        XCTAssertEqual(calls.map(\.name), ["bash", "bash", "job_output"])
        XCTAssertEqual(calls.map(\.state), [.done, .done, .done])
        XCTAssertEqual(calls[0].output, "fn main() {\n    println!(\"hello\");\n}")
        XCTAssertEqual(ToolPresentation.detail(calls[0].name, input: calls[0].input, cwd: "/Users/dev/src/demo"), "cat main.rs")

        let spawn = try XCTUnwrap(items.lazy.compactMap { if case .subagent(let call) = $0 { return call }; return nil }.first)
        XCTAssertEqual(spawn.label, "reader")
        XCTAssertEqual(spawn.child, Fixtures.reader, "the id comes from the report header")
        XCTAssertEqual(spawn.status(child: nil), .completed)
        XCTAssertEqual(spawn.report(child: nil), "First line of `README.md`, verbatim:\n\n```\n# demo\n```")

        guard case .assistant(let answer) = items.last else { return XCTFail("the answer comes last") }
        XCTAssertTrue(answer.markdown.hasPrefix("## Summary"))
        XCTAssertEqual(items.filter { if case .thinking = $0 { return true }; return false }.count, 3)
    }

    func testLiveEvents() throws {
        let state = try liveState()
        let transcript = Transcript(history: [], log: state.log(Fixtures.main), after: -1, pending: [], children: state.children(of: Fixtures.main))
        let items = transcript.items
        XCTAssertNil(transcript.draft, "every turn ended")

        let calls = tools(items)
        XCTAssertEqual(calls.map(\.name), ["bash", "bash", "job_output", "bash"])
        XCTAssertEqual(calls.map(\.state), [.done, .done, .done, .denied])
        XCTAssertEqual(calls.last?.output, "Denied")
        XCTAssertEqual(calls[1].millis, 0)

        let spawn = try XCTUnwrap(items.lazy.compactMap { if case .subagent(let call) = $0 { return call }; return nil }.first)
        XCTAssertEqual(spawn.child, Fixtures.reader)
        XCTAssertTrue(spawn.started)
        XCTAssertEqual(spawn.status(child: state.agents[Fixtures.reader]), .completed)

        let ends = items.compactMap { if case .turnEnd(let summary) = $0 { return summary }; return nil }
        XCTAssertEqual(ends.map(\.status), [.completed, .completed])
        XCTAssertGreaterThan(ends[0].usage?.inputTokens ?? 0, 0)
        XCTAssertGreaterThan(ends[0].milliseconds ?? 0, 0)
        XCTAssertGreaterThan(ends[0].contextTokens, 0)
        XCTAssertEqual(items.filter { if case .user = $0 { return true }; return false }.count, 2)
    }

    func testAgentsCreatedWhileConnectedNeedNoHistory() throws {
        let state = try liveState()
        XCTAssertTrue(state.sawCreation(of: Fixtures.reader))
        XCTAssertTrue(state.sawCreation(of: Fixtures.second))
        var restored = DaemonState()
        restored.apply(.snapshot(try Fixtures.decode(Snapshot.self, "rest/snapshot.json")))
        XCTAssertFalse(restored.sawCreation(of: Fixtures.main), "known from the snapshot only")
    }

    func testCancelledTurn() throws {
        let state = try liveState()
        let items = Transcript(history: [], log: state.log(Fixtures.second), after: -1, pending: [], children: []).items
        guard case .user(let prompt) = items.first else { return XCTFail("prompt") }
        XCTAssertEqual(prompt.text, "Describe this image in one sentence, then use bash to run `sleep 20 && echo done`.\n")
        XCTAssertEqual(prompt.attachments.map(\.path), ["pixel.png"])
        let call = try XCTUnwrap(tools(items).first)
        XCTAssertEqual(call.state, .failed)
        XCTAssertEqual(call.output, "cancelled by the user")
        guard case .turnEnd(let end) = items.last else { return XCTFail("turn end") }
        XCTAssertEqual(end.status, .cancelled)
    }

    func testStreamingDraftAndApproval() throws {
        let messages = try Fixtures.received()
        var transcript = Transcript()
        var sawDraft = false
        var sawApproval = false
        for message in messages {
            guard case .event(_, _, let envelope) = message, envelope.agent == Fixtures.main else { continue }
            transcript.apply(envelope)
            if let draft = transcript.draft, !draft.thinking.isEmpty { sawDraft = true }
            if tools(transcript.items).contains(where: { $0.state == .awaitingApproval && $0.requestId != nil }) { sawApproval = true }
        }
        XCTAssertTrue(sawDraft)
        XCTAssertTrue(sawApproval)
    }

    func testPendingRequestsAreClaimed() throws {
        let history = [
            Message(role: .user, content: [.text("list files")]),
            Message(role: .assistant, content: [.toolUse(id: "call_1", name: "bash", input: .object(["command": .string("ls")]))]),
        ]
        let request = PendingApproval(id: "r1", callId: "", agent: "a", agentLabel: "main", tool: "bash", summary: "ls",
                                      input: .object(["command": .string("ls")]), tab: nil)
        let transcript = Transcript(history: history, log: [], after: 0, pending: [request], children: [])
        let call = try XCTUnwrap(tools(transcript.items).first)
        XCTAssertEqual(call.state, .awaitingApproval, "matched by tool and input when the call id is missing")
        XCTAssertEqual(call.requestId, "r1")
    }

    func testAttachmentsAndRecalledMemories() {
        let history = [Message(role: .user, content: [
            .text("<recalled-memories>\nThe user prefers tabs.\n</recalled-memories>"),
            .text("<attachment path=\"/src/notes.txt\">\nhello\n</attachment>"),
            .text("summarize the notes"),
        ])]
        let items = Transcript(history: history, log: [], after: 0, pending: [], children: []).items
        guard case .recalled(_, let memories) = items.first else { return XCTFail("recalled") }
        XCTAssertEqual(memories, "The user prefers tabs.")
        guard case .user(let message) = items.last else { return XCTFail("user") }
        XCTAssertEqual(message.text, "summarize the notes")
        XCTAssertEqual(message.attachments, [Attachment(path: "/src/notes.txt", body: "hello")])
    }

    func testPlansSupersedeEachOther() {
        func todo(_ id: String, _ status: String) -> Block {
            .toolUse(id: id, name: "todo", input: .object(["items": .array([.object(["content": .string("write tests"), "status": .string(status)])])]))
        }
        let items = Transcript(history: [Message(role: .assistant, content: [todo("t1", "in_progress"), todo("t2", "completed")])],
                               log: [], after: 0, pending: [], children: []).items
        let plans = items.compactMap { if case .plan(let plan) = $0 { return plan }; return nil }
        XCTAssertEqual(plans.map(\.superseded), [true, false])
        XCTAssertEqual(plans.last?.completed, 1)
    }

    func testToolPresentation() {
        XCTAssertEqual(ToolPresentation.title("bash").title, "Bash")
        XCTAssertEqual(ToolPresentation.title("github__create_issue").title, "create issue")
        XCTAssertEqual(ToolPresentation.title("github__create_issue").tag, "github")
        XCTAssertEqual(ToolPresentation.title("browser_click").title, "Click")
        XCTAssertEqual(ToolPresentation.title("browser_click").tag, "browser")
        let read = JSONValue.object(["path": .string("/src/app/main.rs"), "offset": .number(10), "limit": .number(20)])
        XCTAssertEqual(ToolPresentation.detail("read", input: read, cwd: "/src/app"), "main.rs:10+20")
        let grep = JSONValue.object(["pattern": .string("fn main"), "path": .string("/src/app/src")])
        XCTAssertEqual(ToolPresentation.detail("grep", input: grep, cwd: "/src/app"), "fn main in src")
        XCTAssertEqual(ToolPresentation.summary("agent_wait", input: .object([:])), "all subagents")
    }

    func testLineDiff() {
        let lines = LineDiff.lines(from: "a\nb\nc\nd", to: "a\nB\nc\nd\ne")
        XCTAssertEqual(lines.map(\.op), [.same, .removed, .added, .same, .same, .added])
        XCTAssertEqual(LineDiff.stats(lines).added, 2)
        let long = (1 ... 20).map(String.init).joined(separator: "\n")
        let rows = LineDiff.rows(LineDiff.lines(from: long, to: long.replacingOccurrences(of: "\n10\n", with: "\nten\n")))
        XCTAssertEqual(rows.first, .gap(6))
        XCTAssertEqual(rows.last, .gap(7))
        XCTAssertEqual(rows.count, 2 + 3 + 2 + 3)
    }
}
