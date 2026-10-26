package io.malac.core.transcript

import io.malac.core.Fixtures
import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.Message
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.SessionTranscript
import io.malac.core.protocol.TurnStatus
import io.malac.core.state.AgentState
import io.malac.core.state.DaemonState
import io.malac.core.state.LogEntry
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
}
