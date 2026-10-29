package io.malac.core.state

import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.ServerMessage
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Pushes as `crates/malac/src/daemon/push.rs` sends them, and the background check they lead to. */
class PushTest {
    @Test
    fun approvalsWithDetailsAreNotifiedAtOnce() {
        val push = parsePush(
            """{"type":"approvals","requests":[
                {"id":"r1","agent":"a1","agent_label":"main","tool":"bash","summary":"cargo test","tab":"t1","place":"malac · work"},
                {"id":"r2","agent":"a1"}
            ]}""".encodeToByteArray(),
        ) as Push.Approvals
        val pushed = push.notify.single()
        assertEquals("r1", pushed.approval.id)
        assertEquals("bash", pushed.approval.tool)
        assertEquals("cargo test", pushed.approval.summary)
        assertEquals("main", pushed.approval.agentLabel)
        assertEquals("t1", pushed.approval.tab)
        assertEquals("malac · work", pushed.place)
        assertTrue(push.fetch, "r2 came without details")
    }

    @Test
    fun moreMeansFetch() {
        val push = parsePush("""{"type":"approvals","requests":[],"more":true}""".encodeToByteArray()) as Push.Approvals
        assertTrue(push.notify.isEmpty())
        assertTrue(push.fetch)
        val complete = parsePush(
            """{"type":"approvals","requests":[{"id":"r1","agent":"a1","tool":"edit"}]}""".encodeToByteArray(),
        ) as Push.Approvals
        assertEquals(false, complete.fetch)
    }

    @Test
    fun resolvedListsIds() {
        assertEquals(Push.Resolved(setOf("r1", "r2")), parsePush("""{"type":"resolved","requests":["r1","r2"]}""".encodeToByteArray()))
    }

    @Test
    fun unknownPushesCallForAFetch() {
        assertNull(parsePush("""{"type":"later"}""".encodeToByteArray()))
        assertNull(parsePush("not json".encodeToByteArray()))
        assertNull(parsePush(ByteArray(0)))
    }

    @Test
    fun theCheckReportsRequestsAnsweredElsewhere() {
        val snapshot = MalacJson.decodeFromString(
            ServerMessage.serializer(),
            """{"type":"snapshot","pending":[{"id":"r2","agent":"a1","tool":"bash"}]}""",
        ) as ServerMessage.Snapshot
        val result = check(snapshot, Ledger(requests = setOf("r1", "r2")))
        assertEquals(setOf("r1"), result.resolved)
        assertTrue(result.approvals.isEmpty(), "r2 was notified already")
        assertEquals(setOf("r2"), result.ledger.requests)
    }
}
