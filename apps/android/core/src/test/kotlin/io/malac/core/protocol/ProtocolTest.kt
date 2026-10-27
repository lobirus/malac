package io.malac.core.protocol

import io.malac.core.Fixtures
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ProtocolTest {
    @Test
    fun everyRecordedMessageDecodesToAKnownType() {
        val messages = Fixtures.receivedMessages
        assertEquals(Fixtures.received.size, messages.size)
        val unknown = messages.filterIsInstance<ServerMessage.Unknown>()
        assertTrue(unknown.isEmpty(), "unknown messages: $unknown")
        val events = messages.filterIsInstance<ServerMessage.Event>()
        val unknownEvents = events.map { it.envelope.event }.filterIsInstance<AgentEvent.Unknown>()
        assertTrue(unknownEvents.isEmpty(), "unknown events: $unknownEvents")
        assertTrue(events.map { it.envelope.seq }.zipWithNext().all { (a, b) -> a < b }, "seq increases")
    }

    @Test
    fun recordedEventsCarryTheirFields() {
        val events = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Event>().map { it.envelope }
        val request = events.map { it.event }.filterIsInstance<AgentEvent.PermissionRequested>().first()
        assertEquals("bash", request.tool)
        assertEquals("cat main.rs", request.summary)
        assertEquals(JsonPrimitive("cat main.rs"), request.input.jsonObject["command"])

        val spawn = events.map { it.event }.filterIsInstance<AgentEvent.ToolFinished>().first { it.tool == "agent_spawn" }
        assertTrue(spawn.output.startsWith("Report from `reader` (id ${Fixtures.READER})"))
        assertEquals(1537, spawn.millis)

        val usage = events.map { it.event }.filterIsInstance<AgentEvent.UsageReport>().first()
        assertEquals(3665, usage.turn.inputTokens)
        assertEquals(0.00061025, usage.total.cost, 1e-12)
        assertEquals(3786, usage.contextTokens)

        val cancelled = events.filter { it.agent == Fixtures.SECOND }.map { it.event }.filterIsInstance<AgentEvent.TurnFinished>().single()
        assertEquals(TurnStatus.Cancelled, cancelled.status)

        val started = events.first()
        assertEquals(Instant.parse("2026-10-26T06:51:37.318517Z"), started.at)
        assertEquals(AgentEvent.AgentStarted("main", null, "lora/fast", "/Users/dev/src/demo"), started.event)
    }

    @Test
    fun repliesKeepTheirClientRef() {
        val messages = Fixtures.receivedMessages
        val ack = messages.filterIsInstance<ServerMessage.Ack>().first()
        assertEquals("r1", ack.clientRef)
        assertEquals(Fixtures.TAB, ack.tab)
        assertEquals(Fixtures.WORKSPACE, ack.workspace)
        val output = messages.filterIsInstance<ServerMessage.JobOutputReply>().single()
        assertEquals("19959044", output.output.job.id)
        assertEquals(JobStatus.Running, output.output.job.status)
        // `mode` is answered with an ack; the broadcast names the tab and its workspace.
        val mode = messages.filterIsInstance<ServerMessage.ModeChanged>().single()
        assertEquals(ServerMessage.ModeChanged(mode = Mode.Edits, tab = Fixtures.TAB, workspace = Fixtures.WORKSPACE), mode)
        assertEquals("r8", messages.filterIsInstance<ServerMessage.Pong>().single().clientRef)
        assertEquals(ServerMessage.Shutdown("stopped by a client"), messages.last())
    }

    @Test
    fun documentationSamplesDecode() {
        val samples = MalacJson.decodeFromString(ListSerializer(ServerMessage.serializer()), Fixtures.text("protocol-doc.json"))
        val snapshot = assertIs<ServerMessage.Snapshot>(samples[0])
        val tab = snapshot.workspace!!.tabs.single()
        assertEquals(Mode.Ask, tab.mode)
        assertEquals(TabOptions(model = "lora/smart", mode = Mode.Ask), tab.options)
        assertEquals("add pagination", tab.snapshots[1].title)
        assertEquals(Usage(inputTokens = 7012, outputTokens = 20), tab.snapshots[0].usage)
        assertEquals("call_1", snapshot.pending.single().callId)

        val update = assertIs<ServerMessage.Event>(samples[2])
        assertNull(update.tab)
        assertEquals(AgentEvent.Update(UpdateStatus("0.1.2", "0.1.3", available = true)), update.envelope.event)

        val ack = assertIs<ServerMessage.Ack>(samples[3])
        assertEquals(AttachmentInfo("/src/app/notes.txt", "text", 27), ack.attachments.single())
        assertNull(assertIs<ServerMessage.Launched>(samples[4]).agent)
        assertEquals(ServerMessage.ModeChanged(mode = Mode.Autopilot, workspace = "01a17450-0000-7000-8000-000000000001"), samples[10])
        assertEquals(ServerMessage.Reloaded, samples[13])
        assertEquals(ServerMessage.Lagged(12), samples[14])
        assertEquals(ServerMessage.Unknown("pairing_requested"), samples.last())
    }

    @Test
    fun aSecondClientSeesTheOpenRequestInItsSnapshot() {
        val snapshot = MalacJson.decodeFromString(ServerMessage.serializer(), Fixtures.text("snapshot-pending.json"))
        snapshot as ServerMessage.Snapshot
        val pending = snapshot.pending.single()
        assertEquals(Fixtures.MAIN, pending.agent)
        assertEquals("call_01a176fa9f6471e5b9c94673", pending.callId)
        assertEquals(Fixtures.TAB, pending.tab)
        val agent = snapshot.workspace!!.tabs.single().snapshots.single()
        assertEquals(AgentStatus.Running, agent.status)
        assertEquals("waiting for approval: bash", agent.activity)
    }

    @Test
    fun restResponsesDecode() {
        val sessions = MalacJson.decodeFromString(SessionList.serializer(), Fixtures.text("rest/sessions.json")).sessions
        assertEquals(listOf(Fixtures.MAIN, Fixtures.READER), sessions.map { it.id })
        // chrono writes nanoseconds here.
        assertEquals(Instant.parse("2026-10-26T06:51:44.434082473Z"), sessions[0].updated)
        assertEquals(Fixtures.MAIN, sessions[1].parent)

        val transcript = MalacJson.decodeFromString(SessionTranscript.serializer(), Fixtures.text("rest/session-main.json"))
        assertEquals("main", transcript.meta.label)
        assertEquals(10, transcript.history.size)
        val result = transcript.history[2].content.single()
        assertIs<Block.ToolResult>(result)
        assertEquals("fn main() {\n    println!(\"hello\");\n}", result.text)

        val output = MalacJson.decodeFromString(JobOutput.serializer(), Fixtures.text("rest/job-output.json"))
        assertEquals("tick 1\ntick 2", output.text)
        assertNull(output.job.tab)

        val snapshot = MalacJson.decodeFromString(ServerMessage.Snapshot.serializer(), Fixtures.text("rest/snapshot.json"))
        assertEquals("lora/fast", snapshot.defaultModel)
        assertEquals(JobStatus.Killed, snapshot.jobs.single().status)
    }

    @Test
    fun unknownValuesFallBack() {
        val message = MalacJson.decodeFromString(
            ServerMessage.serializer(),
            """{"type":"event","tab":null,"envelope":{"agent":"a","seq":1,"at":"2026-01-01T00:00:00Z","type":"turn_finished","status":"exploded","text":"x"}}""",
        ) as ServerMessage.Event
        assertEquals(AgentEvent.TurnFinished(TurnStatus.Completed, "x"), message.envelope.event)
        val block = MalacJson.decodeFromString(Block.serializer(), """{"type":"redacted_thinking","data":"…"}""")
        assertEquals(Block.Unknown("redacted_thinking"), block)
        assertEquals(ServerMessage.Unknown("malformed"), decodeServerMessage("{not json"))
    }

    @Test
    fun commandsEncodeLikeTheWebUi() {
        fun decode(text: String) = MalacJson.parseToJsonElement(text).jsonObject

        assertEquals(
            decode("""{"type":"prompt","text":"hi","tab":"t","images":[{"media_type":"image/jpeg","data":"AAA","name":"photo.jpg"}],"model":"lora/fast","client_ref":"a1"}"""),
            decode(Command.Prompt("hi", tab = "t", images = listOf(ImageInput("image/jpeg", "AAA", "photo.jpg")), model = "lora/fast").encode("a1")),
        )
        assertEquals(
            decode("""{"type":"approve","request_id":"r","decision":"allow_session"}"""),
            decode(Command.Approve("r", Decision.AllowSession).encode()),
        )
        assertEquals(decode("""{"type":"prompt","text":"go","agent":"x"}"""), decode(Command.Prompt("go", agent = "x").encode()))
        assertEquals(decode("""{"type":"snapshot","workspace":"w"}"""), decode(Command.Resync("w").encode()))
        assertEquals(decode("""{"type":"snapshot"}"""), decode(Command.Resync().encode()))
        assertEquals(decode("""{"type":"job_list"}"""), decode(Command.JobList.encode()))
        assertEquals(decode("""{"type":"mode","mode":"autopilot","tab":"t"}"""), decode(Command.SetMode(Mode.Autopilot, "t").encode()))
        assertEquals(decode("""{"type":"job_output","job":"j","tail":400}"""), decode(Command.JobOutputRequest("j", 400).encode()))
    }

    @Test
    fun voiceMessagesDecode() {
        val snapshot = decodeServerMessage(
            """{"type":"snapshot","daemon":{"version":"1.2.0"},"voice":{""" +
                """"dictation":{"enabled":true,"model":"fake/whisper-1","configured":null,"voice":null,"models":["fake/whisper-1"]},""" +
                """"speech":{"enabled":false,"model":"system","configured":"system","voice":null,"models":["fake/tts-1","system"]},""" +
                """"talk":{"enabled":true,"model":null,"configured":null,"voice":"marin","models":[]},""" +
                """"language":"en","voices":["alloy","marin"],"checked":"2026-10-27T14:02:11.123Z"}}""",
        )
        val voice = assertIs<ServerMessage.Snapshot>(snapshot).voice!!
        assertTrue(voice.dictation.ready)
        assertEquals(listOf("fake/tts-1", "system"), voice.speech.models)
        assertTrue(!voice.speech.ready, "off")
        assertTrue(!voice.talk.ready, "no model")
        assertEquals("marin", voice.talk.voice)
        assertEquals("en", voice.language)
        assertEquals(Instant.parse("2026-10-27T14:02:11.123Z"), voice.checked)
        assertNull(assertIs<ServerMessage.Snapshot>(decodeServerMessage("""{"type":"snapshot"}""")).voice, "an older daemon")

        val broadcast = assertIs<ServerMessage.Voice>(decodeServerMessage("""{"type":"voice","voice":{"dictation":{"enabled":false},"speech":{"enabled":true},"talk":{"enabled":true}}}"""))
        assertEquals(VoiceFeature(enabled = true), broadcast.voice.speech)
        assertEquals(
            ServerMessage.Say("c1", "malac needs your approval to run cargo publish.", "approval", "t", "a", 3),
            decodeServerMessage("""{"type":"say","id":"c1","text":"malac needs your approval to run cargo publish.","reason":"approval","tab":"t","agent":"a","listener":3}"""),
        )
        assertEquals(ServerMessage.VoiceListening("a4", 7), decodeServerMessage("""{"type":"voice_listening","client_ref":"a4","listener":7}"""))
        assertEquals(ServerMessage.VoiceListening("a5", null), decodeServerMessage("""{"type":"voice_listening","client_ref":"a5","listener":null}"""))
        assertEquals(
            MalacJson.parseToJsonElement("""{"type":"voice_listen","on":true,"client_ref":"a1"}"""),
            MalacJson.parseToJsonElement(Command.VoiceListen(true).encode("a1")),
        )
        assertEquals(MalacJson.parseToJsonElement("""{"type":"voice_listen","on":false}"""), MalacJson.parseToJsonElement(Command.VoiceListen(false).encode()))
    }

    @Test
    fun talkMessagesDecode() {
        assertEquals(TalkMessage.Ready("fake/gpt-realtime", "marin"), decodeTalkMessage("""{"type":"ready","model":"fake/gpt-realtime","voice":"marin"}"""))
        assertEquals(TalkMessage.Listening, decodeTalkMessage("""{"type":"listening"}"""))
        assertEquals(TalkMessage.Thinking, decodeTalkMessage("""{"type":"thinking"}"""))
        assertEquals(TalkMessage.Transcript(Role.User, "say hello", true), decodeTalkMessage("""{"type":"transcript","role":"user","text":"say hello","final":true}"""))
        assertEquals(TalkMessage.Transcript(Role.Assistant, "Hel", false), decodeTalkMessage("""{"type":"transcript","role":"assistant","text":"Hel","final":false}"""))
        assertEquals(TalkMessage.Tool("ask_malac", "Asked malac: say hello"), decodeTalkMessage("""{"type":"tool","name":"ask_malac","summary":"Asked malac: say hello"}"""))
        assertEquals(TalkMessage.Done, decodeTalkMessage("""{"type":"done"}"""))
        assertEquals(TalkMessage.Error("no realtime model"), decodeTalkMessage("""{"type":"error","message":"no realtime model"}"""))
        assertEquals(TalkMessage.Closed("talk mode ended"), decodeTalkMessage("""{"type":"closed","reason":"talk mode ended"}"""))
        assertEquals(TalkMessage.Unknown("session_created"), decodeTalkMessage("""{"type":"session_created","id":1}"""))
        assertEquals(TalkMessage.Unknown("malformed"), decodeTalkMessage("{nope"))
        assertEquals("""{"type":"text","text":"hi"}""", TalkCommand.Text("hi").encode())
        assertEquals("""{"type":"interrupt"}""", TalkCommand.Interrupt.encode())
        assertEquals("""{"type":"stop"}""", TalkCommand.Stop.encode())
    }

    @Test
    fun recordedCommandsRoundTrip() {
        val sent = Fixtures.sessionLines.filter { it["dir"]?.jsonPrimitive?.content == "out" }.map { it.getValue("msg").jsonObject }
        val approve = sent.first { it["type"] == JsonPrimitive("approve") }
        val requestId = (approve["request_id"] as JsonPrimitive).content
        assertEquals(JsonObject(approve), MalacJson.parseToJsonElement(Command.Approve(requestId, Decision.Allow).encode()))
    }
}
