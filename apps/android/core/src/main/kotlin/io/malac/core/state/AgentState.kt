package io.malac.core.state

import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.Todo
import io.malac.core.protocol.Usage
import java.time.Instant

/** What the app knows about one agent, from snapshots and its events. */
data class AgentState(
    val id: String,
    val label: String = "agent",
    /** The first prompt, which names the conversation. */
    val title: String = "",
    val parent: String? = null,
    val tab: String? = null,
    val model: String = "",
    val cwd: String = "",
    val created: Instant? = null,
    val status: AgentStatus = AgentStatus.Idle,
    /** What it does right now, e.g. the running tool. */
    val activity: String = "",
    val usage: Usage = Usage.Zero,
    val contextTokens: Long = 0,
    val report: String? = null,
    val todos: List<Todo> = emptyList(),
    val readOnly: Boolean = false,
    /** Loaded in the daemon, as opposed to a saved session. */
    val live: Boolean = false,
    /** Running tool calls: call id to "tool summary". */
    val running: Map<String, String> = emptyMap(),
    val turnStarted: Instant? = null,
) {
    /** The title for top-level agents, the label for subagents. */
    val displayName: String get() = if (parent == null && title.isNotEmpty()) title else label

    val isTopLevel: Boolean get() = parent == null
}

/** The status shown for an agent, a tab or a workspace. */
enum class Phase {
    AwaitingApproval,
    Running,
    Done,
    Failed,
    Stopped,
    Idle;

    companion object {
        fun of(status: AgentStatus): Phase = when (status) {
            AgentStatus.Running -> Running
            AgentStatus.Completed -> Done
            AgentStatus.Failed -> Failed
            AgentStatus.Cancelled -> Stopped
            AgentStatus.Idle -> Idle
        }
    }
}
