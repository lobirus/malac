package io.malac.core.protocol

import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonContentPolymorphicSerializer
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

/**
 * Everything the daemon sends on the WebSocket. Replies to a command carry
 * its `client_ref`; broadcasts do not.
 */
@Serializable(with = ServerMessageSerializer::class)
sealed interface ServerMessage {
    val clientRef: String? get() = null

    /** The first message on every connection, and the reply to `snapshot`. */
    @Serializable
    data class Snapshot(
        @SerialName("client_ref") override val clientRef: String? = null,
        val daemon: DaemonStatus = DaemonStatus(),
        val workspaces: List<WorkspaceSummary> = emptyList(),
        /** The connection's workspace; null before any exists. */
        val workspace: WorkspaceView? = null,
        /** Permission requests waiting for an answer, from every tab. */
        val pending: List<PendingApproval> = emptyList(),
        val jobs: List<JobView> = emptyList(),
        val update: UpdateStatus? = null,
        val models: List<String> = emptyList(),
        @SerialName("default_model") val defaultModel: String? = null,
        val remote: RemoteStatus? = null,
        val awake: AwakeStatus? = null,
        /** Null from daemons older than voice support. */
        val voice: VoiceStatus? = null,
    ) : ServerMessage

    @Serializable
    data class Event(
        val tab: String? = null,
        val workspace: String? = null,
        val envelope: Envelope,
    ) : ServerMessage

    @Serializable
    data class Ack(
        @SerialName("client_ref") override val clientRef: String? = null,
        val agent: String? = null,
        val tab: String? = null,
        val workspace: String? = null,
        val attachments: List<AttachmentInfo> = emptyList(),
    ) : ServerMessage

    @Serializable
    data class Error(
        @SerialName("client_ref") override val clientRef: String? = null,
        val message: String = "",
    ) : ServerMessage

    @Serializable
    data class Pong(@SerialName("client_ref") override val clientRef: String? = null) : ServerMessage

    @Serializable
    data class Launched(
        @SerialName("client_ref") override val clientRef: String? = null,
        val workspace: WorkspaceView,
        val tab: String? = null,
        val agent: String? = null,
    ) : ServerMessage

    @Serializable
    data class Workspaces(
        @SerialName("client_ref") override val clientRef: String? = null,
        val workspaces: List<WorkspaceSummary> = emptyList(),
    ) : ServerMessage

    @Serializable
    data class WorkspaceOpened(
        @SerialName("client_ref") override val clientRef: String? = null,
        val workspace: WorkspaceView,
    ) : ServerMessage

    @Serializable
    data class WorkspaceCreated(val workspace: WorkspaceSummary) : ServerMessage

    @Serializable
    data class WorkspaceUpdated(val workspace: WorkspaceSummary) : ServerMessage

    @Serializable
    data class WorkspaceDeleted(val workspace: String) : ServerMessage

    @Serializable
    data class TabCreated(val workspace: String, val index: Int = 0, val tab: TabView) : ServerMessage

    @Serializable
    data class TabUpdated(val workspace: String, val tab: TabView) : ServerMessage

    @Serializable
    data class TabClosed(val workspace: String, val tab: String) : ServerMessage

    @Serializable
    data class TabsReordered(val workspace: String, val tabs: List<String> = emptyList()) : ServerMessage

    @Serializable
    data class JobStarted(val job: JobView) : ServerMessage

    @Serializable
    data class JobFinished(val job: JobView) : ServerMessage

    @Serializable
    data class Jobs(
        @SerialName("client_ref") override val clientRef: String? = null,
        val jobs: List<JobView> = emptyList(),
    ) : ServerMessage

    @Serializable
    data class JobOutputReply(
        @SerialName("client_ref") override val clientRef: String? = null,
        val output: JobOutput,
    ) : ServerMessage

    @Serializable
    data class Notice(
        val level: Level = Level.Info,
        val message: String = "",
        val tab: String? = null,
    ) : ServerMessage

    /** A tab's mode changed, or with `tab` null every tab of `workspace`. */
    @Serializable
    data class ModeChanged(
        @SerialName("client_ref") override val clientRef: String? = null,
        val mode: Mode = Mode.Ask,
        val tab: String? = null,
        val workspace: String? = null,
    ) : ServerMessage

    @Serializable
    data class Clients(val clients: Int = 0) : ServerMessage

    /** A tunnel started, stopped or got its address; a device paired or was revoked. */
    @Serializable
    data class Remote(val remote: RemoteStatus) : ServerMessage

    /** Keep-awake took or released its assertion, or its mode or reason changed. */
    @Serializable
    data class Awake(val awake: AwakeStatus) : ServerMessage

    /** Voice settings changed, or the providers' model lists were read again. */
    @Serializable
    data class Voice(val voice: VoiceStatus) : ServerMessage

    /**
     * A sentence to read aloud, on the connection whose [listener] id it
     * names: that one fetches `/api/voice/say/{id}` and plays it.
     */
    @Serializable
    data class Say(
        val id: String,
        val text: String = "",
        /** `approval`, `agent` (its `say` tool), `finished` or `failed`. */
        val reason: String = "",
        val tab: String? = null,
        val agent: String? = null,
        val listener: Long = 0,
    ) : ServerMessage

    /** The reply to `voice_listen`: this connection's listener id, or null after `on: false`. */
    @Serializable
    data class VoiceListening(
        @SerialName("client_ref") override val clientRef: String? = null,
        val listener: Long? = null,
    ) : ServerMessage

    /** Settings were applied. */
    data object Reloaded : ServerMessage

    /** The connection fell behind and missed `skipped` messages; resync. */
    @Serializable
    data class Lagged(val skipped: Long = 0) : ServerMessage

    /** The daemon is stopping. */
    @Serializable
    data class Shutdown(val reason: String = "") : ServerMessage

    /** A message type this app does not use or does not know yet. */
    data class Unknown(val type: String) : ServerMessage
}

internal object ServerMessageSerializer : JsonContentPolymorphicSerializer<ServerMessage>(ServerMessage::class) {
    override fun selectDeserializer(element: JsonElement): DeserializationStrategy<ServerMessage> =
        when (val type = element.jsonObject["type"]?.jsonPrimitive?.content) {
            "snapshot" -> ServerMessage.Snapshot.serializer()
            "event" -> ServerMessage.Event.serializer()
            "ack" -> ServerMessage.Ack.serializer()
            "error" -> ServerMessage.Error.serializer()
            "pong" -> ServerMessage.Pong.serializer()
            "launched" -> ServerMessage.Launched.serializer()
            "workspaces" -> ServerMessage.Workspaces.serializer()
            "workspace_opened" -> ServerMessage.WorkspaceOpened.serializer()
            "workspace_created" -> ServerMessage.WorkspaceCreated.serializer()
            "workspace_updated" -> ServerMessage.WorkspaceUpdated.serializer()
            "workspace_deleted" -> ServerMessage.WorkspaceDeleted.serializer()
            "tab_created" -> ServerMessage.TabCreated.serializer()
            "tab_updated" -> ServerMessage.TabUpdated.serializer()
            "tab_closed" -> ServerMessage.TabClosed.serializer()
            "tabs_reordered" -> ServerMessage.TabsReordered.serializer()
            "job_started" -> ServerMessage.JobStarted.serializer()
            "job_finished" -> ServerMessage.JobFinished.serializer()
            "jobs" -> ServerMessage.Jobs.serializer()
            "job_output" -> ServerMessage.JobOutputReply.serializer()
            "notice" -> ServerMessage.Notice.serializer()
            "mode" -> ServerMessage.ModeChanged.serializer()
            "clients" -> ServerMessage.Clients.serializer()
            "remote" -> ServerMessage.Remote.serializer()
            "awake" -> ServerMessage.Awake.serializer()
            "voice" -> ServerMessage.Voice.serializer()
            "say" -> ServerMessage.Say.serializer()
            "voice_listening" -> ServerMessage.VoiceListening.serializer()
            "reloaded" -> UnknownSerializer(type) { ServerMessage.Reloaded }
            "lagged" -> ServerMessage.Lagged.serializer()
            "shutdown" -> ServerMessage.Shutdown.serializer()
            else -> UnknownSerializer(type.orEmpty()) { ServerMessage.Unknown(it) }
        }
}

/** Decodes one WebSocket text frame; a malformed frame becomes [ServerMessage.Unknown]. */
fun decodeServerMessage(text: String): ServerMessage =
    try {
        MalacJson.decodeFromString(ServerMessageSerializer, text)
    } catch (error: IllegalArgumentException) {
        ServerMessage.Unknown("malformed")
    }
