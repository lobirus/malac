import XCTest
@testable import MalacKit

final class VoiceTests: XCTestCase {
    private func decode(_ json: String) throws -> ServerMessage {
        try ServerMessage.decode(Data(json.utf8))
    }

    private static let status = #"""
    {"dictation":{"enabled":true,"model":"fake/whisper-1","configured":null,"voice":null,"models":["fake/whisper-1"]},
     "speech":{"enabled":true,"model":"system","configured":"system","voice":null,"models":["fake/tts-1","system"]},
     "talk":{"enabled":false,"model":"fake/gpt-realtime","configured":null,"voice":"marin","models":["fake/gpt-realtime"]},
     "language":"en","voices":["alloy","marin"],"checked":"2026-10-27T18:44:01.123Z"}
    """#

    func testVoiceMessagesDecode() throws {
        guard case .voice(let status) = try decode(#"{"type":"voice","voice":\#(Self.status)}"#) else { return XCTFail("voice") }
        XCTAssertTrue(status.dictation.isReady)
        XCTAssertEqual(status.dictation.model, "fake/whisper-1")
        XCTAssertEqual(status.speech.configured, "system")
        XCTAssertEqual(status.speech.models, ["fake/tts-1", "system"])
        XCTAssertFalse(status.talk.isReady, "off, though a model could talk")
        XCTAssertEqual(status.talk.voice, "marin")
        XCTAssertEqual(status.language, "en")
        XCTAssertEqual(status.voices, ["alloy", "marin"])
        XCTAssertNotNil(status.checked)

        let older = try JSONDecoder.malac().decode(VoiceStatus.self, from: Data(#"{"dictation":{"enabled":true,"model":null}}"#.utf8))
        XCTAssertFalse(older.dictation.isReady, "no model can do it")
        XCTAssertEqual(older.talk, .unavailable)

        let say = try decode(#"{"type":"say","id":"c1","text":"malac needs your approval to run cargo publish.","reason":"approval","tab":"t1","agent":null,"listener":3}"#)
        guard case .say(let line) = say else { return XCTFail("say") }
        XCTAssertEqual(line, SpokenLine(id: "c1", text: "malac needs your approval to run cargo publish.", reason: "approval", tab: "t1", agent: nil, listener: 3))

        let listening = try decode(#"{"type":"voice_listening","client_ref":"m4","listener":7}"#)
        guard case .voiceListening("m4", 7) = listening else { return XCTFail("voice_listening") }
        XCTAssertEqual(listening.clientRef, "m4", "a reply, matched by its reference")
        guard case .voiceListening(nil, nil) = try decode(#"{"type":"voice_listening","listener":null}"#) else { return XCTFail("off") }
    }

    func testSnapshotsCarryVoice() throws {
        let snapshot = #"{"type":"snapshot","daemon":{"version":"1.2.0","pid":1},"voice":\#(Self.status)}"#
        guard case .snapshot(let value) = try decode(snapshot) else { return XCTFail("snapshot") }
        XCTAssertEqual(value.voice?.dictation.model, "fake/whisper-1")
        let old = try Fixtures.decode(Snapshot.self, "rest/snapshot.json")
        XCTAssertNil(old.voice, "a daemon without voice offers none")
    }

    func testVoiceCommandsEncode() throws {
        let listen = try JSONSerialization.jsonObject(with: Command.voiceListen(on: true).encoded(clientRef: "m1")) as! NSDictionary
        XCTAssertEqual(listen, ["type": "voice_listen", "on": true, "client_ref": "m1"])
        let off = try JSONSerialization.jsonObject(with: Command.voiceListen(on: false).encoded()) as! NSDictionary
        XCTAssertEqual(off, ["type": "voice_listen", "on": false])

        let body = try VoiceSetting.body([.dictation(false), .speechModel("fake/tts-1"), .talkVoice(nil), .language("")])
        XCTAssertEqual(String(decoding: body, as: UTF8.self),
                       #"{"dictation":false,"language":null,"speech_model":"fake/tts-1","talk_voice":null}"#,
                       "null and empty both remove a setting")
    }

    func testOnlyThisConnectionsSentencesPlay() throws {
        var state = DaemonState()
        state.apply(try decode(#"{"type":"snapshot","daemon":{"version":"1.2.0","pid":1},"voice":\#(Self.status)}"#))
        XCTAssertEqual(state.voice?.speech.model, "system")
        let line = #"{"type":"say","id":"c1","text":"Done.","reason":"finished","listener":3}"#
        XCTAssertEqual(state.apply(try decode(line)), [], "not listening yet")

        state.apply(try decode(#"{"type":"voice_listening","client_ref":"m1","listener":3}"#))
        XCTAssertEqual(state.listener, 3)
        XCTAssertEqual(state.apply(try decode(#"{"type":"say","id":"c0","text":"Elsewhere.","reason":"agent","listener":2}"#)), [])
        guard case .speak(let spoken)? = state.apply(try decode(line)).first else { return XCTFail("speak") }
        XCTAssertEqual(spoken.id, "c1")

        state.apply(.snapshot(try Fixtures.decode(Snapshot.self, "rest/snapshot.json")))
        XCTAssertEqual(state.listener, 3, "a resync on the same connection keeps listening")
        XCTAssertNil(state.voice)
        state.forgetListener()
        XCTAssertEqual(state.apply(try decode(line)), [], "a new connection listens only after voice_listen")

        guard case .voice(let changed) = try decode(#"{"type":"voice","voice":\#(Self.status)}"#) else { return XCTFail("voice") }
        state.apply(.voice(changed))
        XCTAssertEqual(state.voice, changed)
    }

    // MARK: Shared files

    func testSharedTags() throws {
        let text = """
        Sent to the user: Screenshot 2026-10-27 14.02.11.jpg, report.pdf.
        <caption>The settings page after the fix</caption>
        <shared id="0193f0c2aa" name="Screenshot 2026-10-27 14.02.11.jpg" type="image/jpeg" size="84211"/>
        <shared id="0193f0c2bb" name="report.pdf" type="application/pdf" size="1200"/>
        """
        let shared = try XCTUnwrap(SharedFiles.parse(text))
        XCTAssertEqual(shared.caption, "The settings page after the fix")
        XCTAssertEqual(shared.files, [
            SharedFile(id: "0193f0c2aa", name: "Screenshot 2026-10-27 14.02.11.jpg", mediaType: "image/jpeg", size: 84211),
            SharedFile(id: "0193f0c2bb", name: "report.pdf", mediaType: "application/pdf", size: 1200),
        ])
        XCTAssertTrue(shared.files[0].isImage)
        XCTAssertFalse(shared.files[1].isImage)
        XCTAssertFalse(SharedFile(id: "a", name: "chart.svg", mediaType: "image/svg+xml", size: 1).isImage, "SVG opens as a file")

        let bare = try XCTUnwrap(SharedFiles.parse(#"Sent to the user: x.<shared id="ab" name="x"/>"#))
        XCTAssertNil(bare.caption)
        XCTAssertEqual(bare.files, [SharedFile(id: "ab", name: "x", mediaType: "application/octet-stream", size: 0)])
        XCTAssertNil(SharedFiles.parse("Sent to the user: nothing."))
        XCTAssertNil(SharedFiles.parse("<shared broken"))
        XCTAssertNil(SharedFiles.parse(#"<shared name="no id"/>"#))
    }

    func testSendToUserCallsCarryTheirFiles() throws {
        let output = "Sent to the user: a.png.\n<shared id=\"0a\" name=\"a.png\" type=\"image/png\" size=\"3\"/>"
        let input = JSONValue.object(["recent_images": .number(1)])
        let history = [
            Message(role: .assistant, content: [.toolUse(id: "s1", name: "send_to_user", input: input), .toolUse(id: "s2", name: "send_to_user", input: input)]),
            Message(role: .user, content: [
                .toolResult(toolUseId: "s1", content: [.text(output)], isError: false),
                .toolResult(toolUseId: "s2", content: [.text(output)], isError: true),
            ]),
        ]
        let calls = Transcript(history: history, log: [], after: 0, pending: [], children: []).items.compactMap { item -> ToolCall? in
            if case .tool(let call) = item { return call }
            return nil
        }
        XCTAssertEqual(calls.map(\.shared?.files.first?.id), ["0a", nil], "a failed call shows as an error")

        var live = Transcript()
        let at = Date()
        live.apply(Envelope(agent: "a", seq: 1, at: at, event: .assistantMessage(Message(role: .assistant, content: [.toolUse(id: "s3", name: "send_to_user", input: input)]))))
        live.apply(Envelope(agent: "a", seq: 2, at: at, event: .toolStarted(callId: "s3", tool: "send_to_user", summary: "", input: input)))
        live.apply(Envelope(agent: "a", seq: 3, at: at, event: .toolFinished(callId: "s3", tool: "send_to_user", output: output, isError: false, millis: 4)))
        guard case .tool(let call)? = live.items.last else { return XCTFail("tool") }
        XCTAssertEqual(call.shared?.files.map(\.name), ["a.png"])
    }

    // MARK: Audio

    func testWAVHeader() throws {
        let wav = WAV.encode([0, 1, -1, Int16.max, Int16.min], rate: PCM16.dictationRate)
        XCTAssertEqual(wav.count, 44 + 10)
        XCTAssertEqual(String(decoding: wav[0 ..< 4], as: UTF8.self), "RIFF")
        XCTAssertEqual(String(decoding: wav[8 ..< 16], as: UTF8.self), "WAVEfmt ")
        XCTAssertEqual(Array(wav[4 ..< 8]), [46, 0, 0, 0], "RIFF size")
        XCTAssertEqual(Array(wav[20 ..< 24]), [1, 0, 1, 0], "PCM, mono")
        XCTAssertEqual(Array(wav[24 ..< 28]), [0x80, 0x3E, 0, 0], "16000 Hz")
        XCTAssertEqual(Array(wav[28 ..< 32]), [0x00, 0x7D, 0, 0], "32000 bytes a second")
        XCTAssertEqual(Array(wav[32 ..< 36]), [2, 0, 16, 0], "2-byte frames, 16 bits")
        XCTAssertEqual(String(decoding: wav[36 ..< 40], as: UTF8.self), "data")
        XCTAssertEqual(Array(wav[40 ..< 44]), [10, 0, 0, 0])
        XCTAssertEqual(Array(wav[44...]), [0, 0, 1, 0, 0xFF, 0xFF, 0xFF, 0x7F, 0x00, 0x80], "little-endian samples")

        let decoded = try XCTUnwrap(WAV.decode(wav))
        XCTAssertEqual(decoded.rate, 16000)
        XCTAssertEqual(decoded.samples, [0, 1, -1, Int16.max, Int16.min])

        // Other writers add chunks such as LIST before the data.
        var padded = Data(wav[0 ..< 36])
        padded.append(contentsOf: Array("LIST".utf8) + [3, 0, 0, 0, 1, 2, 3, 0])
        padded.append(wav[36...])
        XCTAssertEqual(WAV.decode(padded)?.samples, [0, 1, -1, Int16.max, Int16.min])
        XCTAssertNil(WAV.decode(Data("not a wav file".utf8)))
    }

    func testPCMHelpers() {
        let samples: [Int16] = [0, 256, -2, 12345]
        XCTAssertEqual(PCM16.samples(PCM16.data(samples)), samples)
        XCTAssertEqual(PCM16.samples(Data([1, 0, 2])), [1], "an odd byte is dropped")
        XCTAssertEqual(PCM16.samples(Data([9, 1, 0, 2, 0]).dropFirst()), [1, 2], "slices read from their start")
        XCTAssertEqual(PCM16.samples([0, 0.5, -1, 2] as [Float]), [0, 16384, -32767, 32767], "clamped")
        XCTAssertEqual(PCM16.floats(PCM16.data([32767, 0]))[0], 1, accuracy: 1e-6)

        XCTAssertEqual(PCM16.level([Int16](repeating: 0, count: 100)), 0)
        XCTAssertEqual(PCM16.level([Int16](repeating: .max, count: 100)), 1, accuracy: 1e-4)
        let quiet = PCM16.level([Int16](repeating: 328, count: 100)) // -40 dBFS
        XCTAssertEqual(quiet, 0.2, accuracy: 0.01)
        XCTAssertEqual(PCM16.level([Int16]()), 0)
        XCTAssertEqual(PCM16.duration(bytes: 48000, rate: PCM16.talkRate), 1)

        var frames = PCMFrames()
        XCTAssertEqual(frames.frameBytes, 4800, "100 ms at 24 kHz")
        XCTAssertEqual(frames.append(Data(count: 3000)), [])
        let out = frames.append(Data(count: 7000))
        XCTAssertEqual(out.map(\.count), [4800, 4800])
        XCTAssertEqual(frames.flush()?.count, 400)
        XCTAssertNil(frames.flush())
    }

    // MARK: Talk mode

    func testTalkFrames() throws {
        func event(_ json: String) -> TalkEvent? { TalkEvent.decode(Data(json.utf8)) }
        XCTAssertEqual(event(#"{"type":"ready","model":"fake/gpt-realtime","voice":"marin"}"#), .ready(model: "fake/gpt-realtime", voice: "marin"))
        XCTAssertEqual(event(#"{"type":"transcript","role":"user","text":"hi","final":true}"#), .transcript(role: .user, text: "hi", final: true))
        XCTAssertEqual(event(#"{"type":"tool","name":"ask_malac","summary":"Asked malac: say hi"}"#), .tool(name: "ask_malac", summary: "Asked malac: say hi"))
        XCTAssertEqual(event(#"{"type":"error","message":"the voice model failed"}"#), .error("the voice model failed"))
        XCTAssertEqual(event(#"{"type":"closed","reason":"talk mode ended"}"#), .closed("talk mode ended"))
        XCTAssertEqual(event(#"{"type":"wave"}"#), .unknown(type: "wave"))
        XCTAssertNil(event("not json"))

        XCTAssertEqual(TalkCommand.text("run the tests").json, #"{"text":"run the tests","type":"text"}"#)
        XCTAssertEqual(TalkCommand.interrupt.json, #"{"type":"interrupt"}"#)
        XCTAssertEqual(TalkCommand.stop.json, #"{"type":"stop"}"#)

        let endpoint = DaemonEndpoint(base: URL(string: "https://example.test/")!, token: "t")
        XCTAssertEqual(endpoint.talkURL(tab: "t1", agent: "a 1").absoluteString, "wss://example.test/api/voice/talk?tab=t1&agent=a%201")
        let local = DaemonEndpoint(base: URL(string: "http://127.0.0.1:7700/")!, token: "t")
        XCTAssertEqual(local.talkURL(tab: nil, agent: nil).absoluteString, "ws://127.0.0.1:7700/api/voice/talk")
    }

    func testTalkConversation() {
        var talk = TalkState()
        XCTAssertEqual(talk.phase, .connecting)
        talk.apply(.ready(model: "fake/gpt-realtime", voice: "marin"))
        XCTAssertEqual(talk.phase, .listening)
        talk.apply(.listening)
        talk.apply(.thinking)
        XCTAssertEqual(talk.phase, .thinking)
        talk.apply(.transcript(role: .user, text: " run the tests ", final: true))
        talk.apply(.tool(name: "ask_malac", summary: "Asked malac: run the tests"))
        talk.apply(.transcript(role: .assistant, text: "Okay, ", final: false))
        talk.playing = true
        XCTAssertEqual(talk.phase, .speaking)
        talk.apply(.transcript(role: .assistant, text: "I asked", final: false))
        XCTAssertEqual(talk.lines.last?.text, "Okay, I asked")
        XCTAssertEqual(talk.lines.last?.final, false)
        talk.apply(.transcript(role: .assistant, text: "Okay, I asked malac.", final: true))
        talk.apply(.done)
        XCTAssertEqual(talk.lines.map(\.kind), [.user, .tool, .voice])
        XCTAssertEqual(talk.lines.map(\.text), ["run the tests", "Asked malac: run the tests", "Okay, I asked malac."])

        talk.apply(.listening)
        XCTAssertFalse(talk.playing, "talking over the voice stops it")
        XCTAssertEqual(talk.phase, .listening)
        talk.apply(.transcript(role: .assistant, text: "", final: false))
        XCTAssertEqual(talk.lines.count, 3, "no empty lines")
        talk.typed("and the linter")
        talk.apply(.error("the voice model failed"))
        XCTAssertEqual(talk.lines.suffix(2).map(\.kind), [.user, .error])

        talk.apply(.closed(""))
        XCTAssertEqual(talk.phase, .ended)
        XCTAssertEqual(talk.endReason, "Talk mode ended.")
        talk.end("Ended here.")
        XCTAssertEqual(talk.endReason, "Talk mode ended.", "the first reason stays")
    }
}
