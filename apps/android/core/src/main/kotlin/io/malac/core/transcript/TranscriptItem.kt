package io.malac.core.transcript

import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.Block
import io.malac.core.protocol.Decision
import io.malac.core.protocol.Level
import io.malac.core.protocol.Todo
import io.malac.core.protocol.TurnStatus
import io.malac.core.protocol.Usage
import io.malac.core.state.AgentState
import kotlinx.serialization.json.JsonElement
import java.time.Instant

/** One row of a transcript. [key] is stable across updates, for list diffing. */
sealed interface TranscriptItem {
    val key: String

    data class User(
        override val key: String,
        val text: String,
        val images: List<Block.Image> = emptyList(),
        val attachments: List<Attachment> = emptyList(),
        val at: Instant? = null,
    ) : TranscriptItem

    data class Assistant(
        override val key: String,
        val markdown: String,
        val at: Instant? = null,
        /** The turn ended while this text was streaming. */
        val interrupted: Boolean = false,
    ) : TranscriptItem

    data class Thinking(override val key: String, val text: String) : TranscriptItem

    /** Memories attached to a prompt by vector recall. */
    data class Recalled(override val key: String, val text: String) : TranscriptItem

    data class Tool(override val key: String, val call: ToolCall) : TranscriptItem

    data class Subagent(override val key: String, val call: SubagentCall) : TranscriptItem

    data class Plan(
        override val key: String,
        val items: List<Todo>,
        /** A later plan replaced this one. */
        val superseded: Boolean = false,
        val error: String? = null,
    ) : TranscriptItem

    data class Compacted(override val key: String, val summary: String) : TranscriptItem

    data class ModelChanged(override val key: String, val model: String) : TranscriptItem

    data class Notice(override val key: String, val level: Level, val message: String) : TranscriptItem

    data class TurnEnd(override val key: String, val summary: TurnSummary) : TranscriptItem
}

/** A file attached to a prompt; [body] and [images] are known only from the saved history. */
data class Attachment(val path: String, val body: String? = null, val images: List<Block.Image> = emptyList())

data class ToolCall(
    val id: String,
    val name: String,
    val input: JsonElement,
    val state: ToolState = ToolState.Pending,
    /** The open permission request, while [state] is [ToolState.AwaitingApproval]. */
    val requestId: String? = null,
    val output: String? = null,
    val millis: Long? = null,
    val images: List<Block.Image> = emptyList(),
    val at: Instant? = null,
    /** What a finished `send_to_user` call sent; null for other tools and failed calls. */
    val shared: SharedFiles? = null,
    /** The user's answer, when the call waited for approval; null while it waits. */
    val decision: Decision? = null,
    /** Where it was answered, when known: a paired device's name, `this computer`, `talk mode`. */
    val decidedBy: String? = null,
) {
    val isError: Boolean get() = state == ToolState.Failed || state == ToolState.Denied
    val isFinished: Boolean get() = state in setOf(ToolState.Done, ToolState.Failed, ToolState.Denied)
}

enum class ToolState { Pending, Running, AwaitingApproval, Done, Failed, Denied, Skipped }

/** An `agent_spawn` call and the subagent it started. */
data class SubagentCall(
    val id: String,
    val label: String,
    val task: String,
    /** The agent type, when the model asked for one other than the label. */
    val agentType: String?,
    val readOnly: Boolean,
    val child: String? = null,
    val started: Boolean = false,
    val done: Boolean = false,
    val isError: Boolean = false,
    val output: String = "",
    val millis: Long? = null,
    /** The turn ended before the subagent started. */
    val abandoned: Boolean = false,
) {
    /** The live child's status when it is loaded, else what the call says. */
    fun status(child: AgentState?): AgentStatus = when {
        child != null && child.live -> child.status
        !done -> if (started) AgentStatus.Running else AgentStatus.Idle
        !isError -> AgentStatus.Completed
        output.startsWith("cancelled") -> AgentStatus.Cancelled
        else -> AgentStatus.Failed
    }

    fun report(child: AgentState?): String =
        child?.report?.takeIf { it.isNotEmpty() } ?: if (done) spawnReport(output) else ""
}

data class TurnSummary(
    val status: TurnStatus,
    val at: Instant,
    val durationMillis: Long? = null,
    val usage: Usage? = null,
    val contextTokens: Long = 0,
)

/** What the agent is streaming right now, shown after the items. */
data class Draft(val text: String = "", val thinking: String = "", val preparingTool: String? = null)
