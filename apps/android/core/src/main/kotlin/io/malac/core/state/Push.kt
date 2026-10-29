package io.malac.core.state

import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.PendingApproval
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.decodeFromJsonElement

/** What a daemon's push says, once the push library has decrypted it. */
sealed interface Push {
    /**
     * Approvals waiting: the ones with details can be notified at once,
     * the rest (and with [more], others not listed) need a fetch.
     */
    data class Approvals(val notify: List<Pushed>, val fetch: Boolean) : Push

    /** Requests a push was sent for and that are now answered. */
    data class Resolved(val requests: Set<String>) : Push
}

/** A request as a push carries it, with the place the notification names. */
data class Pushed(val approval: PendingApproval, val place: String)

@Serializable
private data class Message(
    val type: String = "",
    val requests: List<JsonElement> = emptyList(),
    val more: Boolean = false,
)

@Serializable
private data class Entry(
    val id: String,
    val agent: String = "",
    @SerialName("agent_label") val agentLabel: String? = null,
    val tool: String? = null,
    val summary: String = "",
    val tab: String? = null,
    val place: String = "",
)

/** Reads a push; `null` for one this app does not understand, which calls for a fetch. */
fun parsePush(content: ByteArray): Push? = try {
    val message = MalacJson.decodeFromString(Message.serializer(), content.decodeToString())
    when (message.type) {
        "approvals" -> {
            val entries = message.requests.filterIsInstance<JsonObject>().map { MalacJson.decodeFromJsonElement<Entry>(it) }
            val detailed = entries.filter { it.tool != null && it.agent.isNotEmpty() }
            Push.Approvals(
                notify = detailed.map {
                    val approval = PendingApproval(id = it.id, agent = it.agent, agentLabel = it.agentLabel.orEmpty(), tool = it.tool.orEmpty(), summary = it.summary, tab = it.tab)
                    Pushed(approval, it.place)
                },
                fetch = message.more || detailed.size < entries.size,
            )
        }
        "resolved" -> Push.Resolved(message.requests.filterIsInstance<JsonPrimitive>().map { it.content }.toSet())
        else -> null
    }
} catch (_: SerializationException) {
    null
} catch (_: IllegalArgumentException) {
    null
}
