package io.malac.core.protocol

import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonContentPolymorphicSerializer
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * A text frame of talk mode's WebSocket, `/api/voice/talk`. Audio comes
 * separately, as binary frames of 16-bit little-endian mono PCM at 24 kHz.
 */
@Serializable(with = TalkMessageSerializer::class)
sealed interface TalkMessage {
    /** Start streaming the microphone. */
    @Serializable
    data class Ready(val model: String? = null, val voice: String? = null) : TalkMessage

    /** The user started speaking: stop playing at once. */
    data object Listening : TalkMessage

    /** The user stopped speaking. */
    data object Thinking : TalkMessage

    /** What the user (final only) or the voice said; the voice's deltas are not [final]. */
    @Serializable
    data class Transcript(val role: Role = Role.Assistant, val text: String = "", val final: Boolean = false) : TalkMessage

    /** The voice model asked malac something, checked on it, answered an approval or stopped it. */
    @Serializable
    data class Tool(val name: String = "", val summary: String = "") : TalkMessage

    /** A reply finished. */
    data object Done : TalkMessage

    @Serializable
    data class Error(val message: String = "") : TalkMessage

    /** The conversation ended. */
    @Serializable
    data class Closed(val reason: String = "") : TalkMessage

    data class Unknown(val type: String) : TalkMessage
}

internal object TalkMessageSerializer : JsonContentPolymorphicSerializer<TalkMessage>(TalkMessage::class) {
    override fun selectDeserializer(element: JsonElement): DeserializationStrategy<TalkMessage> =
        when (val type = element.jsonObject["type"]?.jsonPrimitive?.content) {
            "ready" -> TalkMessage.Ready.serializer()
            "listening" -> UnknownSerializer(type) { TalkMessage.Listening }
            "thinking" -> UnknownSerializer(type) { TalkMessage.Thinking }
            "transcript" -> TalkMessage.Transcript.serializer()
            "tool" -> TalkMessage.Tool.serializer()
            "done" -> UnknownSerializer(type) { TalkMessage.Done }
            "error" -> TalkMessage.Error.serializer()
            "closed" -> TalkMessage.Closed.serializer()
            else -> UnknownSerializer(type.orEmpty()) { TalkMessage.Unknown(it) }
        }
}

/** Decodes one text frame; a malformed frame becomes [TalkMessage.Unknown]. */
fun decodeTalkMessage(text: String): TalkMessage =
    try {
        MalacJson.decodeFromString(TalkMessageSerializer, text)
    } catch (error: IllegalArgumentException) {
        TalkMessage.Unknown("malformed")
    }

/** What the client sends talk mode besides audio. */
@Serializable
sealed interface TalkCommand {
    /** Typed input, as if spoken. */
    @Serializable
    @SerialName("text")
    data class Text(val text: String) : TalkCommand

    /** Stop the current reply. */
    @Serializable
    @SerialName("interrupt")
    data object Interrupt : TalkCommand

    /** End talk mode. */
    @Serializable
    @SerialName("stop")
    data object Stop : TalkCommand
}

fun TalkCommand.encode(): String = MalacJson.encodeToString(TalkCommand.serializer(), this)
