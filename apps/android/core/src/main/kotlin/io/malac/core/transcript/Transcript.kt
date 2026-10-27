package io.malac.core.transcript

import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.Block
import io.malac.core.protocol.Envelope
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.Message
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.Role
import io.malac.core.protocol.Todo
import io.malac.core.protocol.TurnStatus
import io.malac.core.protocol.Usage
import io.malac.core.state.AgentState
import io.malac.core.state.LogEntry
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import java.time.Duration
import java.time.Instant

private val ATTACHMENT_START = Regex("""^<attachment path="([^"]*)"[^>]*>""")
private val ATTACHED_LINE = Regex("""\n?\[attached: ([^\]]*)]\s*$""")
private const val RECALLED = "<recalled-memories>"

/**
 * The transcript of one agent: its saved history, then its live events, as
 * rows. Ported from `web/transcript.js`; the daemon's events reach every
 * client, so this is the same view the web UI and the terminal show.
 */
class Transcript {
    private val rows = mutableListOf<TranscriptItem>()
    private val positions = mutableMapOf<String, Int>()
    private val approvals = mutableMapOf<String, String>()
    private val spawnKeys = mutableListOf<String>()
    private val outgoing = mutableListOf<Pair<String, List<Block.Image>>>()
    private var lastPlanKey: String? = null
    private var turn: Turn? = null
    private var counter = 0

    var draft: Draft? = null
        private set

    val items: List<TranscriptItem> get() = rows.toList()

    private data class Turn(val started: Instant, val usage: Usage = Usage.Zero, val context: Long = 0)

    /** The saved history from `GET /api/sessions/{id}`. */
    fun load(history: List<Message>) {
        for (message in history) {
            if (message.role == Role.User) userBlocks(message.content) else assistantBlocks(message.content, null)
        }
    }

    fun apply(entry: LogEntry) = when (entry) {
        is LogEntry.Event -> apply(entry.envelope)
        is LogEntry.ChildStarted -> childStarted(entry.child, entry.label)
    }

    fun apply(envelope: Envelope) {
        val at = envelope.at
        when (val event = envelope.event) {
            is AgentEvent.UserMessage -> {
                val images = claimImages(event.text)
                userMessage(event.text, images, at)
                turn = Turn(at)
            }
            is AgentEvent.TextDelta -> draft = (draft ?: Draft()).let { it.copy(text = it.text + event.text) }
            is AgentEvent.ThinkingDelta -> draft = (draft ?: Draft()).let { it.copy(thinking = it.thinking + event.text) }
            is AgentEvent.ToolCallStreaming -> draft = (draft ?: Draft()).copy(preparingTool = event.tool)
            is AgentEvent.AssistantMessage -> {
                draft = null
                assistantBlocks(event.message.content, at)
            }
            is AgentEvent.ToolStarted -> updateTool(event.callId) {
                if (it.state == ToolState.Pending) it.copy(state = ToolState.Running, at = at) else it.copy(at = at)
            }
            is AgentEvent.ToolFinished -> finish(event.callId, event.output, event.isError, event.millis)
            is AgentEvent.PermissionRequested -> permissionRequested(event.requestId, event.callId, event.tool, event.input)
            is AgentEvent.PermissionResolved -> permissionResolved(event.requestId, event.allowed)
            is AgentEvent.UsageReport -> turn = turn?.let { it.copy(usage = it.usage + event.turn, context = event.contextTokens) }
            is AgentEvent.Compacted -> append(TranscriptItem.Compacted(nextKey(), event.summary))
            is AgentEvent.ModelChanged -> append(TranscriptItem.ModelChanged(nextKey(), event.model))
            is AgentEvent.Notice -> append(TranscriptItem.Notice(nextKey(), event.level, event.message))
            is AgentEvent.TurnFinished -> turnFinished(event.status, at)
            else -> Unit
        }
    }

    /** Shows requests that were open before the transcript loaded inline. */
    fun claim(pending: List<PendingApproval>) {
        for (request in pending) {
            if (request.id !in approvals) permissionRequested(request.id, request.callId, request.tool, request.input)
        }
    }

    /** Links `agent_spawn` calls of saved history to subagents the daemon still knows, by label. */
    fun linkChildren(children: List<AgentState>) {
        val linked = spawnKeys.mapNotNull { spawn(it)?.child }.toSet()
        val candidates = children.filter { it.id !in linked }.sortedBy { it.created }.toMutableList()
        for (key in spawnKeys) {
            val call = spawn(key) ?: continue
            if (call.child != null) continue
            val index = candidates.indexOfFirst { it.label == call.label }
            if (index >= 0) replaceSpawn(key, call.copy(child = candidates.removeAt(index).id))
        }
    }

    /** Images sent from this app: the `user_message` event carries only text, so they are matched back by it. */
    fun expectImages(text: String, images: List<Block.Image>) {
        if (images.isNotEmpty()) outgoing += text to images
    }

    // History

    private fun userBlocks(blocks: List<Block>) {
        val text = mutableListOf<String>()
        val images = mutableListOf<Block.Image>()
        val attachments = mutableListOf<Attachment>()
        var inAttachment = false
        for (block in blocks) {
            when {
                block is Block.ToolResult -> resultOf(block)
                block is Block.Image && inAttachment -> {
                    val last = attachments.removeAt(attachments.lastIndex)
                    attachments += last.copy(images = last.images + block)
                    continue
                }
                block is Block.Image -> images += block
                block is Block.Text && block.text.startsWith(RECALLED) -> append(TranscriptItem.Recalled(nextKey(), recalledText(block.text)))
                block is Block.Text && ATTACHMENT_START.containsMatchIn(block.text) -> {
                    attachments += parseAttachment(block.text)
                    inAttachment = true
                    continue
                }
                block is Block.Text -> text += block.text
            }
            inAttachment = false
        }
        if (text.isNotEmpty() || images.isNotEmpty() || attachments.isNotEmpty()) {
            userMessage(text.joinToString("\n"), images, null, attachments)
        }
    }

    private fun assistantBlocks(blocks: List<Block>, at: Instant?) {
        for (block in blocks) {
            when (block) {
                is Block.Text -> if (block.text.isNotBlank()) append(TranscriptItem.Assistant(nextKey(), block.text, at))
                is Block.Thinking -> if (block.text.isNotBlank()) append(TranscriptItem.Thinking(nextKey(), block.text))
                is Block.ToolUse -> toolUse(block, at)
                else -> Unit
            }
        }
    }

    private fun userMessage(text: String, images: List<Block.Image>, at: Instant?, attachments: List<Attachment> = emptyList()) {
        draft = null
        val match = ATTACHED_LINE.find(text)
        val typed = if (match != null) text.substring(0, match.range.first).trimEnd() else text
        val chips = attachments.ifEmpty {
            match?.groupValues?.get(1)?.split(',')?.map { it.trim() }?.filter { it.isNotEmpty() }?.map { Attachment(it) }.orEmpty()
        }
        append(TranscriptItem.User(nextKey(), typed, images, chips, at))
    }

    // Tool calls

    private fun toolUse(block: Block.ToolUse, at: Instant?) {
        val key = toolKey(block.id)
        if (key in positions) return
        when (block.name) {
            "agent_spawn" -> {
                val input = block.input
                val label = input.string("label") ?: input.string("agent") ?: "agent"
                val agentType = input.string("agent")?.takeIf { it != label }
                append(TranscriptItem.Subagent(key, SubagentCall(block.id, label, input.string("task").orEmpty(), agentType, input.bool("read_only"))))
                spawnKeys += key
            }
            "todo" -> {
                lastPlanKey?.let { previous -> replace(previous) { (it as TranscriptItem.Plan).copy(superseded = true) } }
                append(TranscriptItem.Plan(key, todos(block.input)))
                lastPlanKey = key
            }
            else -> append(TranscriptItem.Tool(key, ToolCall(block.id, block.name, block.input, at = at)))
        }
    }

    private fun finish(callId: String, output: String, isError: Boolean, millis: Long?, images: List<Block.Image> = emptyList()) {
        when (val item = positions[toolKey(callId)]?.let { rows[it] }) {
            is TranscriptItem.Tool -> replace(item.key) {
                val call = item.call
                TranscriptItem.Tool(
                    item.key,
                    call.copy(
                        state = if (isError) ToolState.Failed else ToolState.Done,
                        requestId = null,
                        output = output,
                        millis = millis ?: call.millis,
                        images = images.ifEmpty { call.images },
                        shared = if (call.name == SEND_TO_USER && !isError) parseShared(output).takeIf { it.files.isNotEmpty() } else null,
                    ),
                )
            }
            is TranscriptItem.Subagent -> {
                val call = item.call
                replaceSpawn(
                    item.key,
                    call.copy(
                        done = true,
                        isError = isError,
                        output = output,
                        millis = millis,
                        child = call.child ?: spawnedAgentId(output),
                    ),
                )
            }
            is TranscriptItem.Plan -> if (isError) replace(item.key) { item.copy(error = output.lineSequence().firstOrNull()) }
            else -> Unit
        }
    }

    private fun resultOf(block: Block.ToolResult) {
        finish(block.toolUseId, block.text, block.isError, null, block.content.filterIsInstance<Block.Image>())
    }

    private fun permissionRequested(requestId: String, callId: String, tool: String, input: JsonElement) {
        val key = toolKey(callId).takeIf { (positions[it]?.let(rows::get) as? TranscriptItem.Tool) != null }
            ?: matchCard(tool, input)
            ?: return
        approvals[requestId] = key
        replace(key) { item ->
            item as TranscriptItem.Tool
            item.copy(call = item.call.copy(state = ToolState.AwaitingApproval, requestId = requestId))
        }
    }

    private fun permissionResolved(requestId: String, allowed: Boolean) {
        val key = approvals.remove(requestId) ?: return
        replace(key) { item ->
            item as TranscriptItem.Tool
            val call = item.call
            item.copy(
                call = if (allowed) {
                    call.copy(state = ToolState.Running, requestId = null)
                } else {
                    call.copy(state = ToolState.Denied, requestId = null, output = "Denied")
                },
            )
        }
    }

    // Requests that were open before this transcript loaded may carry no call id.
    private fun matchCard(tool: String, input: JsonElement): String? = rows.asReversed()
        .filterIsInstance<TranscriptItem.Tool>()
        .firstOrNull { !it.call.isFinished && it.call.requestId == null && it.call.name == tool && it.call.input == input }
        ?.key

    private fun updateTool(callId: String, update: (ToolCall) -> ToolCall) {
        when (val item = positions[toolKey(callId)]?.let { rows[it] }) {
            is TranscriptItem.Tool -> replace(item.key) { item.copy(call = update(item.call)) }
            is TranscriptItem.Subagent -> replaceSpawn(item.key, item.call.copy(started = true))
            else -> Unit
        }
    }

    // Subagents

    private fun childStarted(child: String, label: String) {
        val open = spawnKeys.mapNotNull { key -> spawn(key)?.takeIf { it.child == null }?.let { key to it } }
        val target = open.lastOrNull { (_, call) -> call.started && call.label == label }
            ?: open.lastOrNull { (_, call) -> call.started }
            ?: open.lastOrNull()
        if (target != null) {
            replaceSpawn(target.first, target.second.copy(child = child))
            return
        }
        val key = nextKey()
        append(TranscriptItem.Subagent(key, SubagentCall(key, label, "", null, false, child = child, started = true)))
        spawnKeys += key
    }

    private fun spawn(key: String): SubagentCall? = (positions[key]?.let { rows[it] } as? TranscriptItem.Subagent)?.call

    private fun replaceSpawn(key: String, call: SubagentCall) = replace(key) { TranscriptItem.Subagent(key, call) }

    // Turns

    private fun turnFinished(status: TurnStatus, at: Instant) {
        draft?.let { interrupted ->
            if (interrupted.thinking.isNotBlank()) append(TranscriptItem.Thinking(nextKey(), interrupted.thinking))
            if (interrupted.text.isNotBlank()) append(TranscriptItem.Assistant(nextKey(), interrupted.text, at, interrupted = true))
        }
        draft = null
        for (index in rows.indices) {
            rows[index] = when (val item = rows[index]) {
                is TranscriptItem.Tool -> if (item.call.isFinished) item else item.copy(call = item.call.copy(state = ToolState.Skipped, requestId = null))
                is TranscriptItem.Subagent -> if (item.call.done || item.call.child != null) item else item.copy(call = item.call.copy(abandoned = true))
                else -> item
            }
        }
        approvals.clear()
        val current = turn
        val summary = TurnSummary(
            status = status,
            at = at,
            durationMillis = current?.let { Duration.between(it.started, at).toMillis() },
            usage = current?.usage,
            contextTokens = current?.context ?: 0,
        )
        if (status != TurnStatus.Completed || current != null) append(TranscriptItem.TurnEnd(nextKey(), summary))
        turn = null
    }

    // Rows

    private fun append(item: TranscriptItem) {
        positions[item.key] = rows.size
        rows += item
    }

    private fun replace(key: String, update: (TranscriptItem) -> TranscriptItem) {
        val index = positions[key] ?: return
        rows[index] = update(rows[index])
    }

    private fun nextKey() = "row${counter++}"

    private fun toolKey(callId: String) = "call:$callId"

    private fun claimImages(text: String): List<Block.Image> {
        val index = outgoing.indexOfFirst { text.startsWith(it.first) }
        return if (index >= 0) outgoing.removeAt(index).second else emptyList()
    }
}

private val IMAGE_PLACEHOLDER = Regex("""\[image/[\w.+-]+ image]""")

/**
 * Live tool results carry only placeholders for their images, which the
 * saved history has once the next message or the turn's end is recorded.
 * This is the seq of the last such moment in [log] after [cutoff], when
 * fetching the history again would show images it does not have yet.
 */
fun imagesSavedAt(log: List<LogEntry>, cutoff: Long): Long? {
    var waiting = false
    var saved: Long? = null
    for (entry in log) {
        if (entry.seq <= cutoff || entry !is LogEntry.Event) continue
        when (val event = entry.envelope.event) {
            is AgentEvent.ToolFinished -> if (IMAGE_PLACEHOLDER.containsMatchIn(event.output)) waiting = true
            is AgentEvent.AssistantMessage, is AgentEvent.TurnFinished -> if (waiting) {
                saved = entry.seq
                waiting = false
            }
            else -> Unit
        }
    }
    return saved
}

private fun todos(input: JsonElement): List<Todo> {
    val items = (input as? JsonObject)?.get("items") as? JsonArray ?: return emptyList()
    return items.mapNotNull { runCatching { MalacJson.decodeFromJsonElement(Todo.serializer(), it) }.getOrNull() }
}

private fun parseAttachment(text: String): Attachment {
    val match = ATTACHMENT_START.find(text)!!
    val body = text.substring(match.range.last + 1).replace(Regex("""</attachment>\s*$"""), "").trim()
    return Attachment(match.groupValues[1], body.ifEmpty { null })
}

private fun recalledText(text: String): String =
    text.removePrefix(RECALLED).replace(Regex("""\s*</recalled-memories>\s*$"""), "").trim()
