package io.malac.core.voice

import io.malac.core.protocol.Role
import io.malac.core.protocol.TalkMessage

/** One line of a spoken conversation: what the user or the voice said, or what it asked malac. */
sealed interface TalkLine {
    val key: Int

    data class Said(override val key: Int, val role: Role, val text: String, val final: Boolean) : TalkLine

    /** "Asked malac: …", "Checked on malac", … */
    data class Tool(override val key: Int, val name: String, val summary: String) : TalkLine
}

/** Talk mode as the talk screen shows it, built from the daemon's messages. */
data class TalkState(
    val phase: Phase = Phase.Connecting,
    val model: String? = null,
    val voice: String? = null,
    val lines: List<TalkLine> = emptyList(),
    /** The last error, until the next reply. */
    val error: String? = null,
    /** Why the conversation ended, once it has. */
    val ended: String? = null,
    private val nextKey: Int = 0,
) {
    enum class Phase { Connecting, Listening, Thinking, Speaking, Ended }

    /** The microphone streams from `ready` until the end. */
    val streaming: Boolean get() = phase == Phase.Listening || phase == Phase.Thinking || phase == Phase.Speaking

    fun reduce(message: TalkMessage): TalkState = when (message) {
        is TalkMessage.Ready -> copy(phase = Phase.Listening, model = message.model, voice = message.voice)
        TalkMessage.Listening -> copy(phase = Phase.Listening, error = null)
        TalkMessage.Thinking -> copy(phase = Phase.Thinking)
        is TalkMessage.Transcript -> said(message).let { if (message.role == Role.Assistant) it.copy(phase = Phase.Speaking) else it }
        is TalkMessage.Tool -> append(TalkLine.Tool(nextKey, message.name, message.summary))
        TalkMessage.Done -> copy(phase = Phase.Listening)
        is TalkMessage.Error -> copy(error = message.message)
        is TalkMessage.Closed -> ended(message.reason)
        is TalkMessage.Unknown -> this
    }

    /** The voice's audio arrived: it is speaking. */
    fun speaking(): TalkState = if (streaming) copy(phase = Phase.Speaking) else this

    fun ended(reason: String): TalkState = if (phase == Phase.Ended) this else copy(phase = Phase.Ended, ended = reason)

    /** Something on this side failed, such as the microphone. */
    fun failed(message: String): TalkState = copy(error = message)

    // The voice's deltas grow one line until its final transcript replaces
    // it. The user's transcript can arrive after the voice started
    // answering, so it goes before that answer.
    private fun said(message: TalkMessage.Transcript): TalkState {
        if (message.text.isEmpty() && !message.final) return this
        val open = lines.lastOrNull() as? TalkLine.Said
        val answering = open != null && open.role == Role.Assistant && !open.final
        return when {
            message.role == Role.User -> {
                if (message.text.isBlank()) return this
                val line = TalkLine.Said(nextKey, Role.User, message.text.trim(), final = true)
                if (answering) copy(lines = trim(lines.dropLast(1) + line + open!!), nextKey = nextKey + 1) else append(line)
            }
            answering -> {
                val text = if (message.final) message.text.ifEmpty { open!!.text } else open!!.text + message.text
                copy(lines = lines.dropLast(1) + open.copy(text = text, final = message.final))
            }
            message.text.isBlank() -> this
            else -> append(TalkLine.Said(nextKey, Role.Assistant, message.text, message.final))
        }
    }

    private fun append(line: TalkLine): TalkState = copy(lines = trim(lines + line), nextKey = nextKey + 1)

    private fun trim(lines: List<TalkLine>): List<TalkLine> = if (lines.size > MAX_LINES) lines.takeLast(MAX_LINES) else lines

    companion object {
        const val MAX_LINES = 200
    }
}
