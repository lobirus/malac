package io.malac.core.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

@Serializable
data class Usage(
    @SerialName("input_tokens") val inputTokens: Long = 0,
    @SerialName("output_tokens") val outputTokens: Long = 0,
    @SerialName("cache_read_tokens") val cacheReadTokens: Long = 0,
    @SerialName("cache_write_tokens") val cacheWriteTokens: Long = 0,
    /** USD, when the provider reports it. */
    val cost: Double = 0.0,
) {
    operator fun plus(other: Usage) = Usage(
        inputTokens + other.inputTokens,
        outputTokens + other.outputTokens,
        cacheReadTokens + other.cacheReadTokens,
        cacheWriteTokens + other.cacheWriteTokens,
        cost + other.cost,
    )

    companion object {
        val Zero = Usage()
    }
}

/** An agent as the daemon reports it in a tab's `snapshots`. */
@Serializable
data class AgentView(
    val id: String,
    val label: String = "agent",
    val parent: String? = null,
    val depth: Int = 0,
    @SerialName("read_only") val readOnly: Boolean = false,
    val cwd: String = "",
    val created: Timestamp? = null,
    val model: String = "",
    val status: AgentStatus = AgentStatus.Idle,
    val activity: String = "",
    val usage: Usage = Usage.Zero,
    val report: String? = null,
    /** Loaded in the daemon; a saved session loads when it is prompted. */
    val live: Boolean = false,
    /** The first prompt, for saved sessions. */
    val title: String = "",
)

@Serializable
data class TabOptions(
    val model: String? = null,
    val mode: Mode? = null,
    val profile: String? = null,
    @SerialName("no_control") val noControl: Boolean = false,
)

@Serializable
data class TabView(
    val id: String,
    val name: String = "",
    val cwd: String = "",
    /** Top-level agents, in order. */
    val agents: List<String> = emptyList(),
    @SerialName("active_agent") val activeAgent: String? = null,
    val mode: Mode = Mode.Ask,
    val options: TabOptions = TabOptions(),
    val loaded: Boolean = false,
    /** The top-level agents, live or saved, and the live subagents. */
    val snapshots: List<AgentView> = emptyList(),
)

@Serializable
data class WorkspaceSummary(
    val id: String,
    val name: String = "",
    val tabs: Int = 0,
    @SerialName("active_tab") val activeTab: String? = null,
    /** Agents of this workspace working right now. */
    val running: Int = 0,
    val created: Timestamp? = null,
    val updated: Timestamp? = null,
)

@Serializable
data class WorkspaceView(
    val id: String,
    val name: String = "",
    @SerialName("active_tab") val activeTab: String? = null,
    val created: Timestamp? = null,
    val updated: Timestamp? = null,
    val tabs: List<TabView> = emptyList(),
) {
    fun summary(): WorkspaceSummary = WorkspaceSummary(
        id = id,
        name = name,
        tabs = tabs.size,
        activeTab = activeTab,
        running = tabs.sumOf { tab -> tab.snapshots.count { it.status == AgentStatus.Running } },
        created = created,
        updated = updated,
    )
}

/** A tool call waiting for an answer. `callId` matches the `tool_use` block. */
@Serializable
data class PendingApproval(
    val id: String,
    @SerialName("call_id") val callId: String = "",
    val agent: String,
    @SerialName("agent_label") val agentLabel: String = "",
    val tool: String = "",
    val summary: String = "",
    val input: JsonElement = JsonObject(emptyMap()),
    val tab: String? = null,
)

@Serializable
data class JobView(
    val id: String,
    val command: String = "",
    val cwd: String = "",
    val pid: Int? = null,
    val log: String = "",
    val started: Timestamp? = null,
    val finished: Timestamp? = null,
    @SerialName("exit_code") val exitCode: Int? = null,
    val status: JobStatus = JobStatus.Unknown,
    val agent: String? = null,
    val tab: String? = null,
) {
    val isRunning: Boolean get() = status == JobStatus.Running
}

/** The last lines of a job's log. */
@Serializable
data class JobOutput(
    val job: JobView,
    val text: String = "",
    /** Size of the whole log in bytes. */
    val size: Long = 0,
    /** Whether `text` starts after the beginning of the log. */
    val truncated: Boolean = false,
)

@Serializable
data class DaemonStatus(
    val version: String = "",
    val pid: Int = 0,
    val started: Timestamp? = null,
    @SerialName("version_mismatch") val versionMismatch: Boolean = false,
    val clients: Int = 0,
    val attached: Boolean = false,
    val remote: Boolean = false,
    @SerialName("user_home") val userHome: String? = null,
)

@Serializable
data class UpdateStatus(
    val current: String = "",
    val latest: String? = null,
    val available: Boolean = false,
    val installed: String? = null,
)

@Serializable
data class Todo(
    val content: String,
    val status: TodoStatus = TodoStatus.Pending,
)

/** How the daemon read an attachment of a prompt. */
@Serializable
data class AttachmentInfo(
    val path: String,
    val kind: String = "",
    val size: Long = 0,
    val note: String? = null,
)

/** A saved session from `GET /api/sessions`. */
@Serializable
data class SessionSummary(
    val id: String,
    val label: String = "",
    val parent: String? = null,
    val cwd: String = "",
    val model: String = "",
    val created: Timestamp? = null,
    val updated: Timestamp? = null,
    val title: String = "",
)

@Serializable
data class SessionList(val sessions: List<SessionSummary> = emptyList())

@Serializable
data class SessionMeta(
    val id: String,
    val label: String = "",
    val parent: String? = null,
    val cwd: String = "",
    val model: String = "",
    val created: Timestamp? = null,
)

/** `GET /api/sessions/{id}`: a saved transcript. */
@Serializable
data class SessionTranscript(
    val meta: SessionMeta,
    val history: List<Message> = emptyList(),
    val usage: Usage = Usage.Zero,
)

@Serializable
data class DeviceView(
    val id: String,
    val name: String = "",
    val kind: String = "",
    val created: Timestamp? = null,
    @SerialName("last_seen") val lastSeen: Timestamp? = null,
    val address: String? = null,
    val current: Boolean = false,
)

@Serializable
data class DeviceList(val devices: List<DeviceView> = emptyList())

/** `GET /api/hello`, which answers without a token. */
@Serializable
data class Hello(
    val app: String = "",
    val version: String = "",
    /** Whether remote access is on. */
    val remote: Boolean = false,
    val pairing: Boolean = false,
)

/** How other machines reach the daemon. */
@Serializable
data class RemoteStatus(
    /** Remote access is on: paired devices on other machines may connect. */
    val enabled: Boolean = false,
    /** The bound address, e.g. `0.0.0.0:7700`. */
    val listen: String = "",
    /** Addresses other devices can use, best first. */
    val urls: List<RemoteUrl> = emptyList(),
    val tunnel: TunnelStatus? = null,
    /** The number of paired devices. */
    val devices: Int = 0,
)

/** [kind] is `tunnel`, `public`, `lan` or `local`. */
@Serializable
data class RemoteUrl(val kind: String = "", val url: String = "")

@Serializable
data class TunnelStatus(
    /** `cloudflare`, `ngrok`, `tailscale` or `tailscale-funnel`. */
    val provider: String = "",
    /** `starting`, `running`, `failed` or `stopped`. */
    val state: String = "",
    /** The public address, once the tunnel reports it. */
    val url: String? = null,
    val error: String? = null,
    val notes: List<String> = emptyList(),
    val started: Timestamp? = null,
)

/** Whether the daemon keeps its machine awake, and why. */
@Serializable
data class AwakeStatus(
    val mode: KeepAwake = KeepAwake.Busy,
    /** A power assertion is held right now. */
    val awake: Boolean = false,
    /** Why, for people: "2 agents running", "nothing is running". */
    val reason: String = "",
    /** `caffeinate` or `systemd-inhibit`; null where neither exists. */
    val method: String? = null,
    val error: String? = null,
    /** When [awake] last changed. */
    val since: Timestamp? = null,
)

/** One voice feature. It is [ready] when [enabled] and a [model] can do it; clients show its button only then. */
@Serializable
data class VoiceFeature(
    val enabled: Boolean = false,
    /** The model in use: [configured], else the first of [models]; null when none can do it. */
    val model: String? = null,
    /** The model the settings name, if any. */
    val configured: String? = null,
    /** The voice, for speech and talk mode. */
    val voice: String? = null,
    /** Every `provider/model` that can do it; speech may list `system`. */
    val models: List<String> = emptyList(),
) {
    val ready: Boolean get() = enabled && model != null
}

/** Dictation, speech and talk mode: whether each is on and which model it uses. */
@Serializable
data class VoiceStatus(
    val dictation: VoiceFeature = VoiceFeature(),
    val speech: VoiceFeature = VoiceFeature(),
    val talk: VoiceFeature = VoiceFeature(),
    /** The dictation language; null lets the model work it out. */
    val language: String? = null,
    /** Voices speech and talk models know. */
    val voices: List<String> = emptyList(),
    /** When the providers' model lists were last read. */
    val checked: Timestamp? = null,
)

/** `POST /api/voice/transcribe`: what was said, and the model that heard it. */
@Serializable
data class Transcription(val text: String = "", val model: String = "")
