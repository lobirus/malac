package io.malac.core.protocol

import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.descriptors.buildClassSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonContentPolymorphicSerializer
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import java.time.Instant

/** One agent event: `{agent, seq, at, type, ...}`, the event's fields flattened in. */
@Serializable(with = EnvelopeSerializer::class)
data class Envelope(
    val agent: String,
    /** Increases across every tab of the daemon. */
    val seq: Long,
    val at: Instant,
    val event: AgentEvent,
)

/** What an agent did, from `crates/malac-core/src/event.rs`. */
@Serializable(with = AgentEventSerializer::class)
sealed interface AgentEvent {
    @Serializable
    data class AgentStarted(
        val label: String = "agent",
        val parent: String? = null,
        val model: String = "",
        val cwd: String = "",
    ) : AgentEvent

    @Serializable
    data class ModelChanged(val model: String) : AgentEvent

    @Serializable
    data class UserMessage(val text: String = "") : AgentEvent

    @Serializable
    data class TextDelta(val text: String = "") : AgentEvent

    @Serializable
    data class ThinkingDelta(val text: String = "") : AgentEvent

    @Serializable
    data class ToolCallStreaming(val tool: String = "") : AgentEvent

    @Serializable
    data class AssistantMessage(val message: Message) : AgentEvent

    @Serializable
    data class ToolStarted(
        @SerialName("call_id") val callId: String,
        val tool: String = "",
        val summary: String = "",
        val input: JsonElement = JsonObject(emptyMap()),
    ) : AgentEvent

    @Serializable
    data class ToolFinished(
        @SerialName("call_id") val callId: String,
        val tool: String = "",
        val output: String = "",
        @SerialName("is_error") val isError: Boolean = false,
        val millis: Long = 0,
    ) : AgentEvent

    @Serializable
    data class PermissionRequested(
        @SerialName("request_id") val requestId: String,
        @SerialName("call_id") val callId: String = "",
        val tool: String = "",
        val summary: String = "",
        val input: JsonElement = JsonObject(emptyMap()),
    ) : AgentEvent

    @Serializable
    data class PermissionResolved(
        @SerialName("request_id") val requestId: String,
        val allowed: Boolean = false,
        /** How it was answered; null from older daemons. */
        val decision: Decision? = null,
        /** Where it was answered, when known: a paired device's name, `this computer`, `talk mode`. */
        val by: String? = null,
    ) : AgentEvent

    @Serializable
    data class Todos(val items: List<Todo> = emptyList()) : AgentEvent

    @Serializable
    data class UsageReport(
        val turn: Usage = Usage.Zero,
        val total: Usage = Usage.Zero,
        @SerialName("context_tokens") val contextTokens: Long = 0,
    ) : AgentEvent

    @Serializable
    data class Compacted(val summary: String = "") : AgentEvent

    @Serializable
    data class Notice(val level: Level = Level.Info, val message: String = "") : AgentEvent

    @Serializable
    data class TurnFinished(val status: TurnStatus = TurnStatus.Completed, val text: String = "") : AgentEvent

    @Serializable
    data class AgentFinished(val status: TurnStatus = TurnStatus.Completed, val report: String = "") : AgentEvent

    /** Published with an empty agent id when a newer malac is available or installed. */
    @Serializable
    data class Update(val status: UpdateStatus = UpdateStatus()) : AgentEvent

    data class Unknown(val type: String) : AgentEvent
}

internal object AgentEventSerializer : JsonContentPolymorphicSerializer<AgentEvent>(AgentEvent::class) {
    override fun selectDeserializer(element: JsonElement): DeserializationStrategy<AgentEvent> =
        when (val type = element.jsonObject["type"]?.jsonPrimitive?.content) {
            "agent_started" -> AgentEvent.AgentStarted.serializer()
            "model_changed" -> AgentEvent.ModelChanged.serializer()
            "user_message" -> AgentEvent.UserMessage.serializer()
            "text_delta" -> AgentEvent.TextDelta.serializer()
            "thinking_delta" -> AgentEvent.ThinkingDelta.serializer()
            "tool_call_streaming" -> AgentEvent.ToolCallStreaming.serializer()
            "assistant_message" -> AgentEvent.AssistantMessage.serializer()
            "tool_started" -> AgentEvent.ToolStarted.serializer()
            "tool_finished" -> AgentEvent.ToolFinished.serializer()
            "permission_requested" -> AgentEvent.PermissionRequested.serializer()
            "permission_resolved" -> AgentEvent.PermissionResolved.serializer()
            "todos" -> AgentEvent.Todos.serializer()
            "usage" -> AgentEvent.UsageReport.serializer()
            "compacted" -> AgentEvent.Compacted.serializer()
            "notice" -> AgentEvent.Notice.serializer()
            "turn_finished" -> AgentEvent.TurnFinished.serializer()
            "agent_finished" -> AgentEvent.AgentFinished.serializer()
            "update" -> AgentEvent.Update.serializer()
            else -> UnknownSerializer(type.orEmpty()) { AgentEvent.Unknown(it) }
        }
}

internal object EnvelopeSerializer : KSerializer<Envelope> {
    override val descriptor: SerialDescriptor = buildClassSerialDescriptor("io.malac.Envelope")

    override fun deserialize(decoder: Decoder): Envelope {
        val json = decoder as JsonDecoder
        val element = json.decodeJsonElement().jsonObject
        return Envelope(
            agent = element["agent"]?.jsonPrimitive?.content.orEmpty(),
            seq = element["seq"]?.jsonPrimitive?.long ?: 0,
            at = json.json.decodeFromJsonElement(InstantSerializer, element.getValue("at")),
            event = json.json.decodeFromJsonElement(AgentEventSerializer, element),
        )
    }

    override fun serialize(encoder: Encoder, value: Envelope) {
        throw UnsupportedOperationException("envelopes are only received")
    }
}
