package io.malac.core.state

import io.malac.core.format.firstLine
import io.malac.core.format.truncate
import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.AgentView
import io.malac.core.protocol.AwakeStatus
import io.malac.core.protocol.DaemonStatus
import io.malac.core.protocol.Envelope
import io.malac.core.protocol.JobView
import io.malac.core.protocol.Level
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.RemoteStatus
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.TabView
import io.malac.core.protocol.TurnStatus
import io.malac.core.protocol.UpdateStatus
import io.malac.core.protocol.VoiceStatus
import io.malac.core.protocol.WorkspaceSummary
import io.malac.core.protocol.WorkspaceView
import java.time.Instant

/** An entry of an agent's event log, replayed by a transcript after its saved history. */
sealed interface LogEntry {
    val seq: Long

    data class Event(val envelope: Envelope) : LogEntry {
        override val seq: Long get() = envelope.seq
    }

    /** A subagent of this agent started; links it to its `agent_spawn` call. */
    data class ChildStarted(override val seq: Long, val child: String, val label: String, val at: Instant) : LogEntry
}

/** What the app does in response to a message, beyond updating the state. */
sealed interface Effect {
    /** Send `snapshot`, for [workspace] or the connection's. */
    data class Resync(val workspace: String?) : Effect

    data class Notice(val level: Level, val message: String) : Effect

    data class ApprovalRequested(val approval: PendingApproval) : Effect

    /** Answered, here or by another client. */
    data class ApprovalResolved(val requestId: String) : Effect

    /** A top-level agent finished a turn. */
    data class TurnFinished(val agent: AgentState, val status: TurnStatus, val text: String) : Effect

    /** A sentence for this connection to read aloud: fetch `/api/voice/say/{id}` and play it. */
    data class Say(val say: ServerMessage.Say) : Effect

    /** Stop these sentences: the one playing at once, the queued ones before they start. */
    data class SayCancelled(val ids: Set<String>) : Effect
}

data class Reduction(val state: DaemonState, val effects: List<Effect> = emptyList())

/**
 * The daemon as one connection sees it: the first message is a snapshot of
 * the connection's workspace, then broadcasts keep it current. Agents of
 * other workspaces are known from their events; tabs only for the workspace
 * this connection has open.
 */
data class DaemonState(
    val daemon: DaemonStatus? = null,
    val workspaces: List<WorkspaceSummary> = emptyList(),
    val workspace: WorkspaceView? = null,
    val agents: Map<String, AgentState> = emptyMap(),
    /** Requests waiting for an answer, from every workspace, oldest first. */
    val pending: Map<String, PendingApproval> = emptyMap(),
    val jobs: Map<String, JobView> = emptyMap(),
    val models: List<String> = emptyList(),
    val defaultModel: String? = null,
    val update: UpdateStatus? = null,
    val remote: RemoteStatus? = null,
    val awake: AwakeStatus? = null,
    /** Null from daemons older than voice support. */
    val voice: VoiceStatus? = null,
    /** This connection's listener id while it plays speech (`voice_listen`); null on a new connection. */
    val listener: Long? = null,
    /** Set by `shutdown`, cleared by the next snapshot. */
    val shutdownReason: String? = null,
    /** Tab names and workspaces seen so far, to label approvals from other workspaces. */
    val tabs: Map<String, TabRef> = emptyMap(),
    val logs: Map<String, List<LogEntry>> = emptyMap(),
    val lastSeq: Long = -1,
    /** Increases with every snapshot; transcripts reload their history when it changes. */
    val generation: Int = 0,
) {
    data class TabRef(val name: String, val workspace: String?)

    val loaded: Boolean get() = daemon != null

    fun reduce(message: ServerMessage): Reduction = Reducer(this).apply(message)

    // Queries

    fun tab(id: String?): TabView? = workspace?.tabs?.firstOrNull { it.id == id }

    fun log(agent: String): List<LogEntry> = logs[agent].orEmpty()

    fun pendingFor(agent: String): List<PendingApproval> = pending.values.filter { it.agent == agent }

    fun phase(agent: AgentState): Phase = when {
        pending.values.any { it.agent == agent.id } -> Phase.AwaitingApproval
        !agent.live -> Phase.Idle
        else -> Phase.of(agent.status)
    }

    /** The top-level agents of [tab], in order. */
    fun agentsOf(tab: TabView): List<AgentState> = tab.agents.mapNotNull { agents[it] }

    fun childrenOf(agent: String): List<AgentState> =
        agents.values.filter { it.parent == agent }.sortedBy { it.created }

    /** The tab that shows [agent]: its own, or a parent's. */
    fun tabOf(agent: String): TabView? {
        val tabs = workspace?.tabs.orEmpty()
        tabs.firstOrNull { tab -> agent in tab.agents || tab.snapshots.any { it.id == agent } }?.let { return it }
        var current = agents[agent]
        current?.tab?.let { id -> tab(id)?.let { return it } }
        repeat(32) {
            val parent = current?.parent ?: return null
            tabs.firstOrNull { parent in it.agents }?.let { return it }
            current = agents[parent]
        }
        return null
    }

    /** Waiting, running, failed or done, like the web UI's tab badges. */
    fun phase(tab: TabView): Phase {
        val ids = tab.agents.toSet() + tab.snapshots.map { it.id }
        val members = agents.values.filter { it.id in ids || (it.tab == tab.id && it.live) }
        return when {
            pending.values.any { it.tab == tab.id || it.agent in ids } -> Phase.AwaitingApproval
            members.any { it.live && it.status == AgentStatus.Running } -> Phase.Running
            members.any { it.status == AgentStatus.Failed } -> Phase.Failed
            members.any { it.status == AgentStatus.Completed } -> Phase.Done
            else -> Phase.Idle
        }
    }

    fun phase(summary: WorkspaceSummary): Phase {
        val waiting = pending.values.any { approval -> approval.tab?.let { tabs[it]?.workspace } == summary.id }
        return when {
            waiting -> Phase.AwaitingApproval
            summary.running > 0 -> Phase.Running
            else -> Phase.Idle
        }
    }

    /** "tab · workspace" for an approval, as far as it is known. */
    fun place(approval: PendingApproval): String {
        val ref = approval.tab?.let { tabs[it] }
        val workspaceName = ref?.workspace?.let { id -> workspaces.firstOrNull { it.id == id }?.name }
        return listOfNotNull(ref?.name?.takeIf { it.isNotEmpty() }, workspaceName?.takeIf { it.isNotEmpty() })
            .joinToString(" · ")
    }

    companion object {
        const val LOG_LIMIT = 2000
    }
}

/** Applies one message; mirrors `web/store.js`. */
private class Reducer(private var state: DaemonState) {
    private val effects = mutableListOf<Effect>()

    fun apply(message: ServerMessage): Reduction {
        when (message) {
            is ServerMessage.Snapshot -> snapshot(message)
            is ServerMessage.Event -> event(message)
            is ServerMessage.WorkspaceOpened -> setWorkspace(message.workspace)
            is ServerMessage.Launched -> setWorkspace(message.workspace)
            is ServerMessage.Workspaces -> state = state.copy(workspaces = message.workspaces)
            is ServerMessage.WorkspaceCreated -> upsertSummary(message.workspace)
            is ServerMessage.WorkspaceUpdated -> workspaceUpdated(message.workspace)
            is ServerMessage.WorkspaceDeleted -> workspaceDeleted(message.workspace)
            is ServerMessage.TabCreated -> tabCreated(message)
            is ServerMessage.TabUpdated -> tabUpdated(message)
            is ServerMessage.TabClosed -> editWorkspace(message.workspace) { view ->
                view.copy(tabs = view.tabs.filterNot { it.id == message.tab })
            }
            is ServerMessage.TabsReordered -> editWorkspace(message.workspace) { view ->
                val byId = view.tabs.associateBy { it.id }
                view.copy(tabs = message.tabs.mapNotNull { byId[it] })
            }
            is ServerMessage.JobStarted -> jobChanged(message.job)
            is ServerMessage.JobFinished -> jobChanged(message.job)
            is ServerMessage.Jobs -> state = state.copy(jobs = message.jobs.associateBy { it.id })
            is ServerMessage.ModeChanged -> modeChanged(message)
            is ServerMessage.Clients -> state = state.copy(daemon = state.daemon?.copy(clients = message.clients))
            is ServerMessage.Remote -> state = state.copy(remote = message.remote)
            is ServerMessage.Awake -> state = state.copy(awake = message.awake)
            is ServerMessage.Models -> state = state.copy(models = message.models, defaultModel = message.defaultModel)
            is ServerMessage.Voice -> state = state.copy(voice = message.voice)
            is ServerMessage.VoiceListening -> state = state.copy(listener = message.listener)
            // Every client gets every sentence; the one whose listener it names plays it.
            is ServerMessage.Say -> if (state.listener != null && message.listener == state.listener) effects += Effect.Say(message)
            // Whatever the listener now, this connection may still be playing one of them.
            is ServerMessage.SayCancelled -> if (message.ids.isNotEmpty()) effects += Effect.SayCancelled(message.ids.toSet())
            is ServerMessage.Notice -> effects += Effect.Notice(message.level, message.message)
            is ServerMessage.Lagged -> effects += Effect.Resync(state.workspace?.id)
            is ServerMessage.Shutdown -> state = state.copy(shutdownReason = message.reason.ifEmpty { "stopped" })
            is ServerMessage.Error -> if (message.clientRef == null) effects += Effect.Notice(Level.Error, message.message)
            is ServerMessage.Ack,
            is ServerMessage.Pong,
            is ServerMessage.JobOutputReply,
            ServerMessage.Reloaded,
            is ServerMessage.Unknown,
            -> Unit
        }
        return Reduction(state, effects)
    }

    // Snapshot and workspaces

    private fun snapshot(snapshot: ServerMessage.Snapshot) {
        state = state.copy(
            daemon = snapshot.daemon,
            workspaces = snapshot.workspaces,
            pending = snapshot.pending.associateBy { it.id },
            jobs = snapshot.jobs.associateBy { it.id },
            update = snapshot.update ?: state.update,
            remote = snapshot.remote,
            awake = snapshot.awake,
            voice = snapshot.voice,
            models = snapshot.models,
            defaultModel = snapshot.defaultModel,
            shutdownReason = null,
            // Messages may have been missed; transcripts reload their history.
            logs = emptyMap(),
            generation = state.generation + 1,
        )
        setWorkspace(snapshot.workspace)
    }

    private fun setWorkspace(view: WorkspaceView?) {
        state = state.copy(
            workspace = view,
            agents = state.agents.mapValues { (_, agent) -> agent.copy(live = false) },
        )
        view?.tabs?.forEach { applyTab(it, view.id) }
        if (view != null) upsertSummary(view.summary())
    }

    private fun upsertSummary(summary: WorkspaceSummary) {
        val list = state.workspaces
        val index = list.indexOfFirst { it.id == summary.id }
        state = state.copy(workspaces = if (index >= 0) list.toMutableList().also { it[index] = summary } else list + summary)
    }

    private fun workspaceUpdated(summary: WorkspaceSummary) {
        upsertSummary(summary)
        val view = state.workspace
        if (view?.id == summary.id) {
            state = state.copy(workspace = view.copy(name = summary.name, activeTab = summary.activeTab, updated = summary.updated))
        }
    }

    private fun workspaceDeleted(id: String) {
        state = state.copy(workspaces = state.workspaces.filterNot { it.id == id })
        if (state.workspace?.id == id) {
            state = state.copy(workspace = null)
            effects += Effect.Resync(null)
        }
    }

    private fun editWorkspace(id: String, edit: (WorkspaceView) -> WorkspaceView) {
        val view = state.workspace?.takeIf { it.id == id } ?: return
        state = state.copy(workspace = edit(view))
    }

    private fun tabCreated(message: ServerMessage.TabCreated) {
        resyncWithoutWorkspace()
        val view = state.workspace?.takeIf { it.id == message.workspace } ?: return
        if (view.tabs.none { it.id == message.tab.id }) {
            val tabs = view.tabs.toMutableList()
            tabs.add(message.index.coerceIn(0, tabs.size), message.tab)
            state = state.copy(workspace = view.copy(tabs = tabs))
        }
        applyTab(message.tab, message.workspace)
    }

    private fun tabUpdated(message: ServerMessage.TabUpdated) {
        resyncWithoutWorkspace()
        val view = state.workspace?.takeIf { it.id == message.workspace } ?: return
        val index = view.tabs.indexOfFirst { it.id == message.tab.id }
        val tabs = if (index >= 0) view.tabs.toMutableList().also { it[index] = message.tab } else view.tabs + message.tab
        state = state.copy(workspace = view.copy(tabs = tabs))
        applyTab(message.tab, message.workspace)
    }

    // A command can give this connection a workspace it has not seen, as
    // `tab_create` does before any exists; only a snapshot says which.
    private fun resyncWithoutWorkspace() {
        if (state.workspace == null && effects.none { it is Effect.Resync }) effects += Effect.Resync(null)
    }

    private fun applyTab(tab: TabView, workspace: String) {
        state = state.copy(tabs = state.tabs + (tab.id to DaemonState.TabRef(tab.name, workspace)))
        tab.snapshots.forEach { applyAgentView(it, tab.id) }
    }

    private fun applyAgentView(view: AgentView, tab: String) {
        val agent = agent(view.id)
        val title = cleanTitle(view.title)
        putAgent(
            agent.copy(
                label = view.label,
                parent = view.parent,
                tab = tab,
                model = view.model,
                cwd = view.cwd,
                created = view.created,
                status = view.status,
                activity = view.activity,
                usage = view.usage,
                report = view.report,
                readOnly = view.readOnly,
                live = view.live,
                title = title.ifEmpty { agent.title },
            ),
        )
    }

    private fun jobChanged(job: JobView) {
        state = state.copy(jobs = state.jobs + (job.id to job))
    }

    private fun modeChanged(message: ServerMessage.ModeChanged) {
        val view = state.workspace ?: return
        // A mode for every tab names its workspace, which may be another one.
        if (message.tab == null && message.workspace != null && message.workspace != view.id) return
        val tabs = view.tabs.map { if (message.tab == null || it.id == message.tab) it.copy(mode = message.mode) else it }
        state = state.copy(workspace = view.copy(tabs = tabs))
    }

    // Agent events

    private fun event(message: ServerMessage.Event) {
        val envelope = message.envelope
        val event = envelope.event
        if (event is AgentEvent.Update) {
            state = state.copy(update = event.status)
            return
        }
        if (message.tab != null) {
            val known = state.tabs[message.tab]
            if (known?.workspace == null) {
                state = state.copy(tabs = state.tabs + (message.tab to DaemonState.TabRef(known?.name.orEmpty(), message.workspace)))
            }
        }
        state = state.copy(lastSeq = maxOf(state.lastSeq, envelope.seq))
        var agent = agent(envelope.agent).let { if (message.tab != null) it.copy(tab = message.tab) else it }
        if (event is AgentEvent.AgentStarted) agent = agentStarted(agent, envelope, event)
        record(agent.id, LogEntry.Event(envelope))
        agent = track(agent, envelope)
        putAgent(agent)
        trackPermission(agent, envelope, message.tab)
        if (event is AgentEvent.TurnFinished && agent.isTopLevel) {
            effects += Effect.TurnFinished(agent, event.status, event.text)
        }
    }

    private fun agentStarted(agent: AgentState, envelope: Envelope, event: AgentEvent.AgentStarted): AgentState {
        var started = agent.copy(
            label = event.label,
            parent = event.parent,
            model = event.model,
            cwd = event.cwd,
            created = envelope.at,
            status = AgentStatus.Idle,
            live = true,
        )
        val parentId = event.parent ?: return started
        val parent = agent(parentId)
        putAgent(parent)
        if (started.tab == null) started = started.copy(tab = parent.tab)
        record(parentId, LogEntry.ChildStarted(envelope.seq, agent.id, event.label, envelope.at))
        return started
    }

    // Consecutive deltas are merged so long streams stay cheap to replay.
    private fun record(agent: String, entry: LogEntry) {
        val log = state.logs[agent].orEmpty()
        val last = (log.lastOrNull() as? LogEntry.Event)?.envelope
        val next = (entry as? LogEntry.Event)?.envelope
        val merged = if (last != null && next != null) merge(last, next) else null
        val updated = when {
            merged != null -> log.dropLast(1) + LogEntry.Event(merged)
            log.size >= DaemonState.LOG_LIMIT -> log.drop(log.size - DaemonState.LOG_LIMIT + 1) + entry
            else -> log + entry
        }
        state = state.copy(logs = state.logs + (agent to updated))
    }

    private fun merge(last: Envelope, next: Envelope): Envelope? {
        val a = last.event
        val b = next.event
        return when {
            a is AgentEvent.TextDelta && b is AgentEvent.TextDelta -> next.copy(event = AgentEvent.TextDelta(a.text + b.text))
            a is AgentEvent.ThinkingDelta && b is AgentEvent.ThinkingDelta -> next.copy(event = AgentEvent.ThinkingDelta(a.text + b.text))
            else -> null
        }
    }

    private fun track(agent: AgentState, envelope: Envelope): AgentState {
        val tracked = when (val event = envelope.event) {
            is AgentEvent.UserMessage -> agent.copy(
                status = AgentStatus.Running,
                live = true,
                running = emptyMap(),
                turnStarted = envelope.at,
                title = agent.title.ifEmpty { truncate(firstLine(event.text), 80) },
            )
            is AgentEvent.ModelChanged -> agent.copy(model = event.model)
            is AgentEvent.ToolStarted -> agent.copy(running = agent.running + (event.callId to "${event.tool} ${event.summary}".trim()))
            is AgentEvent.ToolFinished -> agent.copy(running = agent.running - event.callId)
            is AgentEvent.UsageReport -> agent.copy(usage = event.total, contextTokens = event.contextTokens)
            is AgentEvent.Todos -> agent.copy(todos = event.items)
            is AgentEvent.TurnFinished -> agent.copy(
                status = event.status.agentStatus,
                report = event.text,
                running = emptyMap(),
                turnStarted = null,
            )
            is AgentEvent.AgentFinished -> agent.copy(status = event.status.agentStatus, report = event.report)
            else -> agent
        }
        return tracked.copy(activity = nextActivity(tracked, envelope.event))
    }

    private fun nextActivity(agent: AgentState, event: AgentEvent): String = when (event) {
        is AgentEvent.UserMessage, is AgentEvent.ThinkingDelta -> "thinking"
        is AgentEvent.TextDelta -> "writing"
        is AgentEvent.ToolCallStreaming -> "preparing ${event.tool}"
        is AgentEvent.ToolStarted, is AgentEvent.ToolFinished, is AgentEvent.PermissionResolved ->
            agent.running.values.lastOrNull() ?: "thinking"
        is AgentEvent.PermissionRequested -> "waiting for approval: ${event.tool}"
        is AgentEvent.TurnFinished -> ""
        else -> agent.activity
    }

    private fun trackPermission(agent: AgentState, envelope: Envelope, tab: String?) {
        when (val event = envelope.event) {
            is AgentEvent.PermissionRequested -> {
                val approval = PendingApproval(
                    id = event.requestId,
                    callId = event.callId,
                    agent = agent.id,
                    agentLabel = agent.label,
                    tool = event.tool,
                    summary = event.summary,
                    input = event.input,
                    tab = tab ?: agent.tab,
                )
                state = state.copy(pending = state.pending + (approval.id to approval))
                effects += Effect.ApprovalRequested(approval)
            }
            is AgentEvent.PermissionResolved -> {
                state = state.copy(pending = state.pending - event.requestId)
                effects += Effect.ApprovalResolved(event.requestId)
            }
            else -> Unit
        }
    }

    // Helpers

    private fun agent(id: String): AgentState = state.agents[id] ?: AgentState(id)

    private fun putAgent(agent: AgentState) {
        state = state.copy(agents = state.agents + (agent.id to agent))
    }
}

/** A session title, unless it is attached content rather than typed text. */
internal fun cleanTitle(title: String?): String = title.orEmpty().takeUnless { it.startsWith("<attachment ") }.orEmpty()

/** Drops a request locally, e.g. after the daemon said it was already answered. */
fun DaemonState.withoutRequest(requestId: String): DaemonState = copy(pending = pending - requestId)

/** One provider's models, as `provider/model` ids. */
data class ModelGroup(val provider: String, val models: List<String>)

/**
 * [models] by provider, the part before the first `/`, in the order the
 * daemon lists them: model pickers show a header per provider.
 */
fun groupModels(models: List<String>): List<ModelGroup> =
    models.groupBy { it.substringBefore('/', "") }.map { (provider, ids) -> ModelGroup(provider, ids) }

/** A model's name under its provider's header: `anthropic/claude-x` reads as `claude-x`. */
fun modelName(model: String): String = model.substringAfter('/')
