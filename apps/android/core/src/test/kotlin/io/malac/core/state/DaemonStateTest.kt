package io.malac.core.state

import io.malac.core.Fixtures
import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.Mode
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.TurnStatus
import io.malac.core.protocol.VoiceFeature
import io.malac.core.protocol.VoiceStatus
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DaemonStateTest {
    private fun replay(messages: List<ServerMessage>, from: DaemonState = DaemonState()): Pair<DaemonState, List<Effect>> =
        messages.fold(from to emptyList()) { (state, effects), message ->
            val reduction = state.reduce(message)
            reduction.state to effects + reduction.effects
        }

    @Test
    fun replayingTheSessionMatchesTheFinalSnapshot() {
        val messages = Fixtures.receivedMessages
        val last = messages.indexOfLast { it is ServerMessage.Snapshot }
        val final = messages[last] as ServerMessage.Snapshot
        val (state, _) = replay(messages.subList(0, last))

        val tab = final.workspace!!.tabs.single()
        assertEquals(tab, state.workspace!!.tabs.single())
        for (view in tab.snapshots) {
            val agent = state.agents.getValue(view.id)
            assertEquals(view.status, agent.status, view.id)
            assertEquals(view.usage, agent.usage, view.id)
            assertEquals(view.parent, agent.parent, view.id)
            assertEquals(view.label, agent.label, view.id)
            assertEquals(view.live, agent.live, view.id)
            assertEquals(view.activity, agent.activity, view.id)
        }
        // The daemon touches `updated` without a broadcast when a tab is renamed.
        assertEquals(final.workspaces.map { it.copy(updated = null) }, state.workspaces.map { it.copy(updated = null) })
        assertEquals(final.jobs.associateBy { it.id }, state.jobs)
        assertTrue(state.pending.isEmpty())
    }

    @Test
    fun tracksAgentsAndEffectsThroughTheSession() {
        val (state, effects) = replay(Fixtures.receivedMessages)

        // The first tab created a workspace this connection had not seen.
        assertEquals(Effect.Resync(null), effects.first())

        assertEquals(4, effects.filterIsInstance<Effect.ApprovalRequested>().size)
        assertEquals(4, effects.filterIsInstance<Effect.ApprovalResolved>().size)
        val turns = effects.filterIsInstance<Effect.TurnFinished>()
        assertEquals(listOf(Fixtures.MAIN, Fixtures.MAIN, Fixtures.SECOND), turns.map { it.agent.id }, "subagent turns are not reported")
        assertEquals(listOf(TurnStatus.Completed, TurnStatus.Completed, TurnStatus.Cancelled), turns.map { it.status })

        val main = state.agents.getValue(Fixtures.MAIN)
        assertTrue(main.title.startsWith("Do these steps in order"))
        assertEquals(main.title, main.displayName)
        val reader = state.agents.getValue(Fixtures.READER)
        assertEquals("reader", reader.displayName)
        assertEquals(Fixtures.TAB, reader.tab)
        assertEquals(listOf(reader), state.childrenOf(Fixtures.MAIN))
        assertEquals(Fixtures.TAB, state.tabOf(Fixtures.READER)?.id)

        val tab = state.workspace!!.tabs.single()
        assertEquals("demo app", tab.name)
        assertEquals(Mode.Edits, tab.mode)
        assertEquals(DaemonState.TabRef("demo app", Fixtures.WORKSPACE), state.tabs[Fixtures.TAB])
        assertEquals("stopped by a client", state.shutdownReason)
        assertEquals(Phase.Stopped, state.phase(state.agents.getValue(Fixtures.SECOND)))
        assertEquals(Phase.Done, state.phase(tab))
    }

    @Test
    fun theEventLogMergesDeltasAndLinksSubagents() {
        val messages = Fixtures.receivedMessages.filterNot { it is ServerMessage.Snapshot && it.clientRef != null }
        val (state, _) = replay(messages)
        val log = state.log(Fixtures.MAIN)
        val events = log.filterIsInstance<LogEntry.Event>().map { it.envelope.event }
        assertTrue(events.zipWithNext().none { (a, b) -> a is AgentEvent.TextDelta && b is AgentEvent.TextDelta })
        val text = events.filterIsInstance<AgentEvent.TextDelta>().first().text
        assertTrue(text.startsWith("## Summary"), text)
        val child = log.filterIsInstance<LogEntry.ChildStarted>().single()
        assertEquals(Fixtures.READER, child.child)
        assertEquals("reader", child.label)
        assertTrue(log.map { it.seq }.zipWithNext().all { (a, b) -> a <= b })
    }

    @Test
    fun anotherClientsAnswerClearsTheRequest() {
        val snapshot = MalacJson.decodeFromString(ServerMessage.serializer(), Fixtures.text("snapshot-pending.json"))
        var state = snapshot.let { DaemonState().reduce(it).state }
        val request = state.pending.values.single()
        val agent = state.agents.getValue(Fixtures.MAIN)
        assertEquals(Phase.AwaitingApproval, state.phase(agent))
        assertEquals(Phase.AwaitingApproval, state.phase(state.workspace!!.tabs.single()))
        assertEquals(Phase.AwaitingApproval, state.phase(state.workspaces.single()))
        assertEquals("demo · default", state.place(request))

        val resolved = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Event>()
            .first { (it.envelope.event as? AgentEvent.PermissionResolved)?.requestId == request.id }
        val reduction = state.reduce(resolved)
        state = reduction.state
        assertTrue(state.pending.isEmpty())
        assertEquals(listOf(Effect.ApprovalResolved(request.id)), reduction.effects)
        assertEquals(Phase.Running, state.phase(state.agents.getValue(Fixtures.MAIN)))
    }

    @Test
    fun laggedAndDeletedWorkspacesResync() {
        val snapshot = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Snapshot>()[1]
        val state = DaemonState().reduce(snapshot).state
        assertEquals(listOf(Effect.Resync(Fixtures.WORKSPACE)), state.reduce(ServerMessage.Lagged(12)).effects)

        val deleted = state.reduce(ServerMessage.WorkspaceDeleted(Fixtures.WORKSPACE))
        assertEquals(listOf(Effect.Resync(null)), deleted.effects)
        assertEquals(null, deleted.state.workspace)
        assertTrue(deleted.state.workspaces.isEmpty())
    }

    @Test
    fun aSnapshotStartsANewGeneration() {
        val messages = Fixtures.receivedMessages
        val first = DaemonState().reduce(messages[0]).state
        val (state, _) = replay(messages.subList(1, 40), first)
        assertTrue(state.log(Fixtures.MAIN).isNotEmpty())
        val again = state.reduce(messages.filterIsInstance<ServerMessage.Snapshot>()[1]).state
        assertEquals(state.generation + 1, again.generation)
        assertTrue(again.logs.isEmpty())
        assertEquals(AgentStatus.Completed, again.agents.getValue(Fixtures.MAIN).status)
    }

    @Test
    fun aModeForEveryTabOfAnotherWorkspaceIsIgnored() {
        val snapshot = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Snapshot>()[1]
        val state = DaemonState().reduce(snapshot).state
        val other = state.reduce(ServerMessage.ModeChanged(mode = Mode.Autopilot, workspace = "elsewhere")).state
        assertEquals(Mode.Ask, other.workspace!!.tabs.single().mode)
        val all = state.reduce(ServerMessage.ModeChanged(mode = Mode.Autopilot, workspace = Fixtures.WORKSPACE)).state
        assertEquals(Mode.Autopilot, all.workspace!!.tabs.single().mode)
    }

    @Test
    fun voiceSettingsFollowTheSnapshotAndBroadcasts() {
        val ready = VoiceStatus(dictation = VoiceFeature(enabled = true, model = "fake/whisper-1"))
        val state = DaemonState().reduce(ServerMessage.Snapshot(voice = ready)).state
        assertEquals(ready, state.voice)
        val off = ready.copy(dictation = ready.dictation.copy(enabled = false))
        assertEquals(off, state.reduce(ServerMessage.Voice(off)).state.voice)
    }

    @Test
    fun onlyThisConnectionsSentencesAreSpoken() {
        val say = ServerMessage.Say("c1", "Done.", "finished", listener = 2)
        val idle = DaemonState().reduce(ServerMessage.Snapshot()).state
        assertTrue(idle.reduce(say).effects.isEmpty(), "not listening")

        val listening = idle.reduce(ServerMessage.VoiceListening("a1", 2)).state
        assertEquals(2L, listening.listener)
        assertEquals(listOf<Effect>(Effect.Say(say)), listening.reduce(say).effects)
        assertTrue(listening.reduce(say.copy(listener = 3)).effects.isEmpty(), "another device's")
        assertEquals(2L, listening.reduce(ServerMessage.Snapshot()).state.listener, "a resync keeps the connection")

        val stopped = listening.reduce(ServerMessage.VoiceListening("a2", null)).state
        assertNull(stopped.listener)
        assertTrue(stopped.reduce(say).effects.isEmpty())
    }

    @Test
    fun cancelledSentencesStopOnEveryConnection() {
        val cancelled = ServerMessage.SayCancelled(listOf("c1", "c2"))
        val idle = DaemonState().reduce(ServerMessage.Snapshot()).state
        assertEquals(listOf<Effect>(Effect.SayCancelled(setOf("c1", "c2"))), idle.reduce(cancelled).effects, "not listening any more")
        val listening = idle.reduce(ServerMessage.VoiceListening("a1", 2)).state
        assertEquals(listOf<Effect>(Effect.SayCancelled(setOf("c1", "c2"))), listening.reduce(cancelled).effects)
        assertTrue(listening.reduce(ServerMessage.SayCancelled()).effects.isEmpty(), "nothing to stop")
    }
}
