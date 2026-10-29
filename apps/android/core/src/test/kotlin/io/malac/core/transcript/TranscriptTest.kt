package io.malac.core.transcript

import io.malac.core.Fixtures
import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.Decision
import io.malac.core.protocol.Envelope
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.Message
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.SessionTranscript
import io.malac.core.protocol.TurnStatus
import io.malac.core.state.AgentState
import io.malac.core.state.DaemonState
import io.malac.core.state.LogEntry
import kotlinx.serialization.json.JsonPrimitive
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class TranscriptTest {
    private val history = MalacJson.decodeFromString(SessionTranscript.serializer(), Fixtures.text("rest/session-main.json")).history

    /** The session without the snapshots it asked for, so every agent's log is complete. */
    private val live: DaemonState by lazy {
        Fixtures.receivedMessages
            .filterNot { it is ServerMessage.Snapshot && it.clientRef != null }
            .fold(DaemonState()) { state, message -> state.reduce(message).state }
    }

    private fun transcriptOf(entries: List<LogEntry>) = Transcript().apply { entries.forEach(::apply) }

    private inline fun <reified T : TranscriptItem> List<TranscriptItem>.only(): List<T> = filterIsInstance<T>()

    @Test
    fun savedHistoryBecomesRows() {
        val items = Transcript().apply { load(history) }.items
        assertEquals(
            listOf("User", "Thinking", "Tool", "Subagent", "Tool", "Thinking", "Tool", "Thinking", "Assistant"),
            items.map { it::class.simpleName },
        )
        val user = items[0] as TranscriptItem.User
        assertTrue(user.text.startsWith("Do these steps in order"))
        val cat = (items[2] as TranscriptItem.Tool).call
        assertEquals("bash", cat.name)
        assertEquals(ToolState.Done, cat.state)
        assertEquals("fn main() {\n    println!(\"hello\");\n}", cat.output)
        val spawn = (items[3] as TranscriptItem.Subagent).call
        assertEquals("reader", spawn.label)
        assertEquals(Fixtures.READER, spawn.child)
        assertEquals("First line of `README.md`, verbatim:\n\n```\n# demo\n```", spawn.report(null))
        assertEquals(AgentStatus.Completed, spawn.status(null))
        assertTrue((items.last() as TranscriptItem.Assistant).markdown.startsWith("## Summary"))
    }

    @Test
    fun liveEventsBuildTheSameRowsPlusTurnFooters() {
        val items = transcriptOf(live.log(Fixtures.MAIN)).items
        val tools = items.only<TranscriptItem.Tool>().map { it.call }
        assertEquals(listOf("bash", "bash", "job_output", "bash"), tools.map { it.name })
        assertEquals(listOf(ToolState.Done, ToolState.Done, ToolState.Done, ToolState.Denied), tools.map { it.state })
        assertEquals("rm main.rs", describeTool(tools[3].name, tools[3].input, "/Users/dev/src/demo"))
        assertEquals(11L, tools[0].millis)

        val spawn = items.only<TranscriptItem.Subagent>().single().call
        assertEquals(Fixtures.READER, spawn.child)
        assertTrue(spawn.done)
        assertEquals(1537L, spawn.millis)

        val footers = items.only<TranscriptItem.TurnEnd>().map { it.summary }
        assertEquals(listOf(TurnStatus.Completed, TurnStatus.Completed), footers.map { it.status })
        val first = footers[0]
        assertTrue(first.durationMillis!! in 1..60_000, "duration ${first.durationMillis}")
        assertTrue(first.usage!!.inputTokens > 3665, "a turn adds up every request")
        assertTrue(first.contextTokens > 0)

        val users = items.only<TranscriptItem.User>().map { it.text }
        assertEquals(2, users.size)
        assertEquals("Use bash to run `rm main.rs`.", users[1])
        assertNull(Transcript().draft)
    }

    @Test
    fun aCancelledTurnEndsWithAStoppedFooter() {
        val items = transcriptOf(live.log(Fixtures.SECOND)).items
        val user = items.only<TranscriptItem.User>().single()
        assertEquals("Describe this image in one sentence, then use bash to run `sleep 20 && echo done`.", user.text)
        assertEquals(listOf(Attachment("pixel.png")), user.attachments)
        val sleep = items.only<TranscriptItem.Tool>().single().call
        assertEquals(ToolState.Failed, sleep.state)
        assertEquals("cancelled by the user", sleep.output)
        assertEquals(TurnStatus.Cancelled, items.only<TranscriptItem.TurnEnd>().single().summary.status)
    }

    @Test
    fun streamingTextIsADraftUntilTheMessageArrives() {
        val log = live.log(Fixtures.READER)
        val transcript = Transcript()
        val draftEnd = log.indexOfFirst { (it as? LogEntry.Event)?.envelope?.event is AgentEvent.TextDelta }
        log.take(draftEnd + 1).forEach(transcript::apply)
        assertTrue(transcript.draft!!.text.startsWith("First line"))
        log.drop(draftEnd + 1).forEach(transcript::apply)
        assertNull(transcript.draft)
        assertTrue(transcript.items.only<TranscriptItem.Assistant>().single().markdown.startsWith("First line"))
    }

    @Test
    fun openRequestsAreClaimedWhenTheTranscriptLoads() {
        val snapshot = MalacJson.decodeFromString(ServerMessage.serializer(), Fixtures.text("snapshot-pending.json")) as ServerMessage.Snapshot
        val request = snapshot.pending.single()
        val beforeRequest = live.log(Fixtures.MAIN).takeWhile { (it as? LogEntry.Event)?.envelope?.event !is AgentEvent.PermissionRequested }

        val transcript = transcriptOf(beforeRequest)
        transcript.claim(listOf(request))
        val call = transcript.items.only<TranscriptItem.Tool>().single().call
        assertEquals(ToolState.AwaitingApproval, call.state)
        assertEquals(request.id, call.requestId)

        // Without a call id the request is matched by tool and input.
        val matched = transcriptOf(beforeRequest)
        matched.claim(listOf(request.copy(callId = "")))
        assertEquals(request.id, matched.items.only<TranscriptItem.Tool>().single().call.requestId)
    }

    @Test
    fun savedAnswersAreMatchedToTheirCalls() {
        val saved = MalacJson.decodeFromString(
            SessionTranscript.serializer(),
            """{"meta":{"id":"a"},"history":[
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"c1","name":"bash","input":{"command":"cargo publish"}},
                    {"type":"tool_use","id":"c2","name":"bash","input":{"command":"rm -rf target"}},
                    {"type":"tool_use","id":"c3","name":"read","input":{"path":"README.md"}}]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"c1","content":[{"type":"text","text":"Published"}]},
                    {"type":"tool_result","tool_use_id":"c2","content":[{"type":"text","text":"The user denied this tool call. Try a different approach or ask the user."}],"is_error":true},
                    {"type":"tool_result","tool_use_id":"c3","content":[{"type":"text","text":"# demo"}]}]}],
             "decisions":[
                {"call_id":"c2","tool":"bash","summary":"rm -rf target","decision":"deny","by":"talk mode","at":"2026-10-29T10:01:00Z"},
                {"call_id":"c1","tool":"bash","summary":"cargo publish","decision":"allow_session","by":"Pixel 9","at":"2026-10-29T10:00:00Z"},
                {"call_id":"gone","tool":"bash","summary":"ls","decision":"allow","at":"2026-10-29T10:02:00Z"}]}""",
        )
        val calls = Transcript().apply { load(saved.history, saved.decisions) }.items.only<TranscriptItem.Tool>().map { it.call }
        assertEquals(listOf(Decision.AllowSession, Decision.Deny, null), calls.map { it.decision })
        assertEquals(listOf("Pixel 9", "talk mode", null), calls.map { it.decidedBy })
        assertEquals(listOf(ToolState.Done, ToolState.Denied, ToolState.Done), calls.map { it.state }, "a denied call reads as denied, not failed")
        assertEquals(ToolState.Failed, Transcript().apply { load(saved.history) }.items.only<TranscriptItem.Tool>()[1].call.state)
    }

    @Test
    fun liveAnswersSayWhoGaveThem() {
        val at = Instant.parse("2026-10-29T10:00:00Z")
        val history = listOf(
            """{"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"bash","input":{"command":"cargo publish"}},""" +
                """{"type":"tool_use","id":"c2","name":"bash","input":{"command":"rm -rf target"}}]}""",
        ).map { MalacJson.decodeFromString(Message.serializer(), it) }
        val transcript = Transcript().apply { load(history) }
        val input = MalacJson.parseToJsonElement("""{"command":"cargo publish"}""")
        transcript.apply(Envelope("a", 1, at, AgentEvent.PermissionRequested("r1", "c1", "bash", "cargo publish", input)))
        transcript.apply(Envelope("a", 2, at, AgentEvent.PermissionRequested("r2", "c2", "bash", "rm -rf target")))
        val waiting = transcript.items.only<TranscriptItem.Tool>().map { it.call }
        assertEquals(listOf(ToolState.AwaitingApproval, ToolState.AwaitingApproval), waiting.map { it.state })
        assertEquals(listOf(null, null), waiting.map { it.decision }, "no answer while it waits")

        transcript.apply(Envelope("a", 3, at, AgentEvent.PermissionResolved("r1", allowed = true, decision = Decision.Allow, by = "this computer")))
        transcript.apply(Envelope("a", 4, at, AgentEvent.PermissionResolved("r2", allowed = false, decision = Decision.Deny, by = "Pixel 9")))
        transcript.apply(Envelope("a", 5, at, AgentEvent.ToolFinished("c1", "bash", "Published", millis = 900)))
        val (allowed, denied) = transcript.items.only<TranscriptItem.Tool>().map { it.call }
        assertEquals(ToolState.Done, allowed.state)
        assertEquals(Decision.Allow to "this computer", allowed.decision to allowed.decidedBy)
        assertEquals(ToolState.Denied, denied.state)
        assertEquals(Decision.Deny to "Pixel 9", denied.decision to denied.decidedBy)
    }

    @Test
    fun anAnswerAfterTheHistoryWasReadFindsItsRow() {
        val history = listOf(
            """{"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"bash","input":{"command":"cargo publish"}}]}""",
        ).map { MalacJson.decodeFromString(Message.serializer(), it) }
        // The request was asked before the cutoff, so only its answer is in the log after it.
        val open = PendingApproval("r1", "c1", "a", "main", "bash", "cargo publish")
        val transcript = Transcript().apply { load(history) }
        transcript.claim(listOf(open))
        transcript.apply(Envelope("a", 9, Instant.parse("2026-10-29T10:00:00Z"), AgentEvent.PermissionResolved("r1", true, Decision.Allow, "Pixel 9")))
        transcript.claim(emptyList())
        val call = transcript.items.only<TranscriptItem.Tool>().single().call
        assertEquals(ToolState.Running, call.state)
        assertEquals(Decision.Allow to "Pixel 9", call.decision to call.decidedBy)
    }

    @Test
    fun subagentsOfSavedHistoryAreLinkedByLabel() {
        val transcript = Transcript()
        transcript.load(history.take(4))
        assertNull(transcript.items.only<TranscriptItem.Subagent>().single().call.child)
        transcript.linkChildren(listOf(AgentState(Fixtures.READER, label = "reader", parent = Fixtures.MAIN)))
        assertEquals(Fixtures.READER, transcript.items.only<TranscriptItem.Subagent>().single().call.child)
    }

    @Test
    fun theLatestPlanStaysOpen() {
        val transcript = Transcript()
        val plan = """{"role":"assistant","content":[{"type":"tool_use","id":"p1","name":"todo","input":{"items":[{"content":"a","status":"completed"},{"content":"b","status":"in_progress"}]}}]}"""
        val again = plan.replace("p1", "p2")
        transcript.load(listOf(plan, again).map { MalacJson.decodeFromString(Message.serializer(), it) })
        val plans = transcript.items.only<TranscriptItem.Plan>()
        assertEquals(listOf(true, false), plans.map { it.superseded })
        assertEquals(2, plans[1].items.size)
    }

    @Test
    fun attachmentsAndRecalledMemoriesInHistory() {
        val message = MalacJson.decodeFromString(
            Message.serializer(),
            """{"role":"user","content":[
                {"type":"text","text":"<recalled-memories>\nuse cargo nextest\n</recalled-memories>"},
                {"type":"text","text":"<attachment path=\"/src/notes.txt\">\nhello\n</attachment>"},
                {"type":"text","text":"summarize the notes"}]}""",
        )
        val items = Transcript().apply { load(listOf(message)) }.items
        assertEquals("use cargo nextest", assertIs<TranscriptItem.Recalled>(items[0]).text)
        val user = assertIs<TranscriptItem.User>(items[1])
        assertEquals("summarize the notes", user.text)
        assertEquals(listOf(Attachment("/src/notes.txt", "hello")), user.attachments)
    }

    @Test
    fun toolRowsReadLikeTheWebUi() {
        val input = MalacJson.parseToJsonElement("""{"path":"/src/app/main.rs","offset":10,"limit":20}""")
        assertEquals("main.rs:10+20", describeTool("read", input, "/src/app"))
        assertEquals(ToolTitle("Read", null), toolTitle("read"))
        assertEquals(ToolTitle("create issue", "github"), toolTitle("github__create_issue"))
        assertEquals(ToolTitle("Click", "browser"), toolTitle("browser_click"))
        assertEquals(ToolTitle("Spawn", null), toolTitle("agent_spawn"))
        val grep = MalacJson.parseToJsonElement("""{"pattern":"TODO","path":"/src/app/src"}""")
        assertEquals("TODO in src", describeTool("grep", grep, "/src/app"))
        assertEquals("2 items", summarizeInput("todo", MalacJson.parseToJsonElement("""{"items":[{},{}]}""")))
    }

    private val sent = "Sent to the user: Screenshot 2026-10-27 14.02.11.jpg, report.pdf.\n" +
        "<caption>The settings page after the fix</caption>\n" +
        "<shared id=\"0193f0c2aa\" name=\"Screenshot 2026-10-27 14.02.11.jpg\" type=\"image/jpeg\" size=\"84211\"/>\n" +
        "<shared id=\"0193f0c2bb\" name=\"report.pdf\" type=\"application/pdf\" size=\"5120\"/>"

    @Test
    fun sharedTagsParseLikeTheDaemonWritesThem() {
        val shared = parseShared(sent)
        assertEquals("The settings page after the fix", shared.caption)
        assertEquals(
            listOf(
                SharedFile("0193f0c2aa", "Screenshot 2026-10-27 14.02.11.jpg", "image/jpeg", 84211),
                SharedFile("0193f0c2bb", "report.pdf", "application/pdf", 5120),
            ),
            shared.files,
        )
        assertEquals(listOf(true, false), shared.files.map { it.isImage })
        assertTrue(!SharedFile("x", "chart.svg", "image/svg+xml", 1).isImage, "drawn by browsers only")
        assertEquals(SharedFiles(null, listOf(SharedFile("x", "a b", "application/octet-stream", 0))), parseShared("""<shared id="x" name="a b"/>"""))
        assertTrue(parseShared("<shared broken").files.isEmpty())
        assertTrue(parseShared("""<shared name="no id"/>""").files.isEmpty())
    }

    @Test
    fun sendToUserCallsCarryTheirFiles() {
        val history = listOf(
            """{"role":"assistant","content":[{"type":"tool_use","id":"s1","name":"send_to_user","input":{"paths":["report.pdf"]}},""" +
                """{"type":"tool_use","id":"b1","name":"bash","input":{"command":"cat notes"}}]}""",
            """{"role":"user","content":[{"type":"tool_result","tool_use_id":"s1","content":[{"type":"text","text":${JsonPrimitive(sent)}}]},""" +
                """{"type":"tool_result","tool_use_id":"b1","content":[{"type":"text","text":"<shared id=\"x\" name=\"y\"/>"}]}]}""",
        ).map { MalacJson.decodeFromString(Message.serializer(), it) }
        val tools = Transcript().apply { load(history) }.items.only<TranscriptItem.Tool>()
        assertEquals(2, tools.single { it.call.name == "send_to_user" }.call.shared?.files?.size)
        assertNull(tools.single { it.call.name == "bash" }.call.shared, "only send_to_user sends files")

        // Live: the files arrive with `tool_finished`.
        val at = Instant.parse("2026-10-27T14:02:11Z")
        val live = Transcript()
        live.load(history.take(1))
        live.apply(Envelope("a", 1, at, AgentEvent.ToolFinished("s1", "send_to_user", sent, millis = 12)))
        live.apply(Envelope("a", 2, at, AgentEvent.ToolFinished("b1", "bash", "", isError = true)))
        val call = live.items.only<TranscriptItem.Tool>().first().call
        assertEquals("The settings page after the fix", call.shared?.caption)
        assertEquals(ToolState.Done, call.state)
        val failed = Transcript().apply { load(history.take(1)) }
        failed.apply(Envelope("a", 1, at, AgentEvent.ToolFinished("s1", "send_to_user", "no such file: report.pdf", isError = true)))
        assertNull(failed.items.only<TranscriptItem.Tool>().first().call.shared)
    }

    @Test
    fun liveToolImagesAreFetchedOnceSaved() {
        val at = Instant.parse("2026-10-27T14:02:11Z")
        fun entry(seq: Long, event: AgentEvent) = LogEntry.Event(Envelope("a", seq, at, event))
        val log = listOf(
            entry(1, AgentEvent.ToolFinished("c1", "bash", "ok")),
            entry(2, AgentEvent.AssistantMessage(Message())),
            entry(3, AgentEvent.ToolFinished("c2", "browser_screenshot", "Took a screenshot.\n[image/png image]")),
            entry(4, AgentEvent.TextDelta("looking")),
            entry(5, AgentEvent.AssistantMessage(Message())),
            entry(6, AgentEvent.TurnFinished()),
        )
        assertNull(imagesSavedAt(log.take(2), cutoff = 0), "no images")
        assertNull(imagesSavedAt(log.take(4), cutoff = 0), "not saved yet")
        assertEquals(5L, imagesSavedAt(log, cutoff = 0))
        assertNull(imagesSavedAt(log, cutoff = 5), "the history fetched after it has them")
    }
}
