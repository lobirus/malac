import XCTest
@testable import MalacKit

/// Dictation, speech, talk mode and shared files through a running daemon
/// whose provider is `crates/malac/examples/fake_voice.rs` (models
/// `whisper-1`, `tts-1`, `gpt-realtime`) and whose chat model fails, so a
/// prompt ends in a failed turn that is read aloud. Skipped unless
/// `MALAC_LIVE_URL` and `MALAC_LIVE_TOKEN` are set; `MALAC_LIVE_CWD` is the
/// project it opens (a temporary directory by default) and
/// `MALAC_LIVE_SHARED` names a file under `~/.malac/shared/` that holds
/// `hello from malac`.
@MainActor
final class LiveVoiceTests: XCTestCase {
    func testVoiceThroughTheDaemon() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let address = environment["MALAC_LIVE_URL"], let token = environment["MALAC_LIVE_TOKEN"],
              let base = URL(string: address.hasSuffix("/") ? address : address + "/") else {
            throw XCTSkip("set MALAC_LIVE_URL and MALAC_LIVE_TOKEN to run against a daemon with the fake voice provider")
        }
        let endpoint = DaemonEndpoint(base: base, token: token)
        let api = DaemonAPI(endpoint: endpoint)
        let cwd = environment["MALAC_LIVE_CWD"] ?? FileManager.default.temporaryDirectory.appending(path: "malackit-live").path
        try FileManager.default.createDirectory(atPath: cwd, withIntermediateDirectories: true)
        let tab = try await Self.createTab(endpoint, cwd: cwd)

        // Settings
        let status = try await api.voice(refresh: true)
        XCTAssertTrue(status.dictation.isReady)
        XCTAssertTrue(status.talk.models.contains { $0.hasSuffix("/gpt-realtime") })
        let german = try await api.setVoice([.language("de")])
        XCTAssertEqual(german.language, "de")
        let automatic = try await api.setVoice([.language(nil)])
        XCTAssertNil(automatic.language, "null removes a setting")
        let restored = try await api.setVoice([.language(status.language)])
        XCTAssertEqual(restored.language, status.language)

        // Dictation: one second of 16 kHz WAV.
        let tone = (0 ..< PCM16.dictationRate).map { Float(sin(Double($0) * 2 * .pi * 440 / Double(PCM16.dictationRate)) * 0.25) }
        let heard = try await api.transcribe(wav: WAV.encode(PCM16.samples(tone), rate: PCM16.dictationRate))
        XCTAssertEqual(heard.text, "This is fake dictation of about 1.0 seconds.")
        XCTAssertTrue(heard.model.hasSuffix("/whisper-1"))

        // Speech: listen, make a turn fail, fetch the sentence.
        let connection = DaemonConnection(endpoint: endpoint)
        var state = DaemonState()
        var spoken: [SpokenLine] = []
        var online = false
        let snapshot = expectation(description: "snapshot")
        let said = expectation(description: "a sentence for this connection")
        connection.onMessage = { message in
            for case .speak(let line) in state.apply(message) where spoken.isEmpty {
                spoken.append(line)
                said.fulfill()
            }
            if case .snapshot = message, !online {
                online = true
                snapshot.fulfill()
            }
        }
        connection.start()
        await fulfillment(of: [snapshot], timeout: 10)
        XCTAssertEqual(state.voice?.speech.isReady, true)
        guard case .voiceListening(_, let listener?) = try await connection.request(.voiceListen(on: true)) else {
            return XCTFail("a listener id")
        }
        XCTAssertEqual(state.listener, listener)
        _ = try await connection.request(.prompt(text: "hello", agent: nil, tab: tab))
        await fulfillment(of: [said], timeout: 30)
        XCTAssertEqual(spoken.first?.reason, "failed")
        let audio = try await api.spoken(try XCTUnwrap(spoken.first).id)
        XCTAssertTrue(["audio/wav", "audio/mpeg"].contains(audio.mediaType))
        XCTAssertGreaterThan(audio.data.count, 1000)

        // Talk mode: a second of audio is an utterance that asks malac something.
        let talk = TalkConnection(endpoint: endpoint, tab: tab, agent: nil)
        var conversation = TalkState()
        var played = 0
        var marks: Set<String> = []
        let ready = expectation(description: "ready")
        let asked = expectation(description: "asked malac")
        let answered = expectation(description: "the voice answered")
        talk.onEvent = { event in
            conversation.apply(event)
            switch event {
            case .ready: ready.fulfill()
            case .tool(_, let summary) where summary.hasPrefix("Asked malac:") && marks.insert("asked").inserted: asked.fulfill()
            case .transcript(.assistant, _, true) where marks.insert("answered").inserted: answered.fulfill()
            default: break
            }
        }
        talk.onAudio = { played += $0.count }
        talk.start()
        await fulfillment(of: [ready], timeout: 15)
        let voice = (0 ..< PCM16.talkRate).map { Float(sin(Double($0) * 2 * .pi * 220 / Double(PCM16.talkRate)) * 0.25) }
        var frames = PCMFrames()
        let chunks = frames.append(PCM16.data(PCM16.samples(voice)))
        XCTAssertEqual(chunks.count, 10)
        for chunk in chunks { talk.send(audio: chunk) }
        await fulfillment(of: [asked, answered], timeout: 30)
        XCTAssertGreaterThan(played, 0, "the voice's audio arrives as binary frames")
        XCTAssertEqual(conversation.lines.first { $0.kind == .user }?.text, "(fake utterance 1)")
        XCTAssertTrue(conversation.lines.contains { $0.kind == .voice && $0.final })
        talk.close()

        guard case .voiceListening(_, nil) = try await connection.request(.voiceListen(on: false)) else { return XCTFail("off") }
        XCTAssertNil(state.listener)
        connection.stop()

        // Shared files
        do {
            _ = try await api.shared("ffffffff")
            XCTFail("an unknown file is 404")
        } catch DaemonError.http(404, _) {}
        if let id = environment["MALAC_LIVE_SHARED"] {
            let file = try await api.shared(id)
            XCTAssertEqual(String(decoding: file.data, as: UTF8.self), "hello from malac")
            XCTAssertEqual(file.mediaType, "text/plain")
            let download = try await api.shared(id, download: true)
            XCTAssertEqual(download.data, file.data)
        }
    }

    /// `tab_create`, which this app does not send otherwise.
    private static func createTab(_ endpoint: DaemonEndpoint, cwd: String) async throws -> String {
        var request = endpoint.request(endpoint.url("api/command"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(JSONValue.object(["type": .string("tab_create"), "cwd": .string(cwd), "reuse": .bool(true)]))
        let (data, _) = try await URLSession.shared.data(for: request)
        guard case .ack(let ack) = try ServerMessage.decode(data), let tab = ack.tab else {
            throw DaemonError.command(String(decoding: data, as: UTF8.self))
        }
        return tab
    }
}
