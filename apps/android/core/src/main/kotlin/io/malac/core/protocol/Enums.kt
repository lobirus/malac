package io.malac.core.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
enum class AgentStatus {
    @SerialName("idle") Idle,
    @SerialName("running") Running,
    @SerialName("completed") Completed,
    @SerialName("failed") Failed,
    @SerialName("cancelled") Cancelled,
}

@Serializable
enum class TurnStatus {
    @SerialName("completed") Completed,
    @SerialName("cancelled") Cancelled,
    @SerialName("failed") Failed;

    val agentStatus: AgentStatus
        get() = when (this) {
            Completed -> AgentStatus.Completed
            Cancelled -> AgentStatus.Cancelled
            Failed -> AgentStatus.Failed
        }
}

/** Permission mode of a tab. */
@Serializable
enum class Mode {
    @SerialName("ask") Ask,
    @SerialName("edits") Edits,
    @SerialName("autopilot") Autopilot,
}

@Serializable
enum class Decision {
    @SerialName("allow") Allow,
    /** Allow this and similar calls for the rest of the session. */
    @SerialName("allow_session") AllowSession,
    @SerialName("deny") Deny,
}

@Serializable
enum class JobStatus {
    @SerialName("running") Running,
    @SerialName("exited") Exited,
    @SerialName("killed") Killed,
    @SerialName("unknown") Unknown,
}

@Serializable
enum class Level {
    @SerialName("info") Info,
    @SerialName("warning") Warning,
    @SerialName("error") Error,
}

/** When the daemon keeps its machine from sleeping. */
@Serializable
enum class KeepAwake {
    /** While agents or jobs run, and a minute after. */
    @SerialName("busy") Busy,
    @SerialName("always") Always,
    @SerialName("off") Off,
}

@Serializable
enum class TodoStatus {
    @SerialName("pending") Pending,
    @SerialName("in_progress") InProgress,
    @SerialName("completed") Completed,
}
