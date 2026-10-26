package io.malac.core.format

import io.malac.core.Fixtures
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.Usage
import io.malac.core.state.Ledger
import io.malac.core.state.check
import io.malac.core.transcript.DiffLine
import io.malac.core.transcript.DiffLine.Op
import io.malac.core.transcript.diffLines
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class FormatTest {
    @Test
    fun numbersReadLikeTheWebUi() {
        assertEquals("950", formatTokens(950))
        assertEquals("7k", formatTokens(7_012))
        assertEquals("27.9k", formatTokens(27_926))
        assertEquals("1.4M", formatTokens(1_400_000))
        assertEquals("$0.0046", formatCost(0.0045699))
        assertEquals("$1.20", formatCost(1.2))
        assertEquals("", formatCost(0.0))
        assertEquals("640ms", formatDuration(640))
        assertEquals("1.5s", formatDuration(1_537))
        assertEquals("3m 04s", formatDuration(184_000))
        assertEquals("2h 05m", formatDuration(7_500_000))
        assertEquals("27.9k in · 762 out", formatUsage(Usage(inputTokens = 27_926, outputTokens = 762)))
        assertEquals("~/src/demo", displayPath("/Users/dev/src/demo", "/Users/dev"))
        assertEquals("/Users/devil", displayPath("/Users/devil", "/Users/dev"))
    }

    @Test
    fun editsDiffByLine() {
        assertEquals(
            listOf(DiffLine(Op.Same, "a"), DiffLine(Op.Removed, "b"), DiffLine(Op.Added, "B"), DiffLine(Op.Same, "c")),
            diffLines("a\nb\nc", "a\nB\nc"),
        )
        assertEquals(listOf(DiffLine(Op.Added, "new")), diffLines("", "new"))
    }

    @Test
    fun backgroundChecksNotifyOncePerRequestAndTurn() {
        val pending = MalacJson.decodeFromString(ServerMessage.serializer(), Fixtures.text("snapshot-pending.json")) as ServerMessage.Snapshot
        val first = check(pending, Ledger())
        assertEquals(1, first.approvals.size)
        assertTrue(first.finished.isEmpty())
        assertEquals(setOf(Fixtures.MAIN), first.ledger.running)

        assertTrue(check(pending, first.ledger).approvals.isEmpty(), "already notified")

        val done = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Snapshot>()[1]
        val later = check(done, first.ledger)
        assertTrue(later.approvals.isEmpty())
        assertEquals(listOf(Fixtures.MAIN), later.finished.map { it.id })
        assertEquals(AgentStatus.Completed, later.finished.single().status)
        assertTrue(later.ledger.requests.isEmpty())
    }
}
