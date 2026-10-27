package io.malac.core.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject

/** A command for the daemon. [encode] adds the `type` and an optional `client_ref`. */
@Serializable
sealed interface Command {
    /** Prompts [agent], or starts a new top-level agent in [tab] with [model]. */
    @Serializable
    @SerialName("prompt")
    data class Prompt(
        val text: String,
        val agent: String? = null,
        val tab: String? = null,
        val images: List<ImageInput> = emptyList(),
        val model: String? = null,
    ) : Command

    @Serializable
    @SerialName("agent_start")
    data class AgentStart(val tab: String, val model: String? = null, val label: String? = null) : Command

    /** Loads a saved session into a tab. */
    @Serializable
    @SerialName("agent_open")
    data class AgentOpen(val agent: String, val tab: String? = null) : Command

    @Serializable
    @SerialName("agent_activate")
    data class AgentActivate(val tab: String, val agent: String) : Command

    /** Cancels the agent's turn and its subagents'. */
    @Serializable
    @SerialName("cancel")
    data class Cancel(val agent: String) : Command

    @Serializable
    @SerialName("approve")
    data class Approve(@SerialName("request_id") val requestId: String, val decision: Decision) : Command

    /** Without [tab], every tab of the connection's workspace. */
    @Serializable
    @SerialName("mode")
    data class SetMode(val mode: Mode, val tab: String? = null) : Command

    @Serializable
    @SerialName("model")
    data class SetModel(val agent: String, val model: String) : Command

    /** With [workspace], switches this connection to it without making it the daemon's last used one. */
    @Serializable
    @SerialName("snapshot")
    data class Resync(val workspace: String? = null) : Command

    @Serializable
    @SerialName("workspace_list")
    data object WorkspaceList : Command

    @Serializable
    @SerialName("job_list")
    data object JobList : Command

    @Serializable
    @SerialName("job_output")
    data class JobOutputRequest(val job: String, val tail: Int? = null) : Command

    @Serializable
    @SerialName("job_kill")
    data class JobKill(val job: String) : Command

    /** Until the daemon restarts; `daemon.keep_awake` is the default. */
    @Serializable
    @SerialName("keep_awake")
    data class SetKeepAwake(val mode: KeepAwake) : Command

    /** Plays speech on this connection, or stops; the reply is `voice_listening`. */
    @Serializable
    @SerialName("voice_listen")
    data class VoiceListen(val on: Boolean) : Command

    @Serializable
    @SerialName("ping")
    data object Ping : Command
}

/** A prompt image: base64 data, shown as an attachment named [name]. */
@Serializable
data class ImageInput(
    @SerialName("media_type") val mediaType: String,
    val data: String,
    val name: String? = null,
)

fun Command.encode(clientRef: String? = null): String {
    val body = MalacJson.encodeToJsonElement(Command.serializer(), this).jsonObject
    val message = if (clientRef == null) body else JsonObject(body + ("client_ref" to JsonPrimitive(clientRef)))
    return message.toString()
}
