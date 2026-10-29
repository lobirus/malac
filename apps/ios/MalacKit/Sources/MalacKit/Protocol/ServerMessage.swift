import Foundation

/// The reply to a command that has none of its own, or to `prompt`,
/// `agent_start`, `tab_create` and friends.
public struct Ack: Hashable, Sendable {
    public var clientRef: String?
    public var agent: String?
    public var tab: String?
    /// The tab's workspace, for commands that create or pick one.
    public var workspace: String?
    public var attachments: [AttachmentInfo]
}

/// Everything the daemon sends over the WebSocket.
public enum ServerMessage: Sendable {
    case snapshot(Snapshot)
    case event(tab: String?, workspace: String?, envelope: Envelope)
    case ack(Ack)
    case error(clientRef: String?, message: String)
    case pong(clientRef: String?)
    case launched(clientRef: String?, workspace: WorkspaceView, tab: String?, agent: String?)
    case workspaces(clientRef: String?, [WorkspaceSummary])
    case workspaceOpened(clientRef: String?, WorkspaceView)
    case workspaceCreated(WorkspaceSummary)
    case workspaceUpdated(WorkspaceSummary)
    case workspaceDeleted(String)
    case tabCreated(workspace: String, index: Int, tab: WorkspaceTab)
    case tabUpdated(workspace: String, tab: WorkspaceTab)
    case tabClosed(workspace: String, tab: String)
    case tabsReordered(workspace: String, tabs: [String])
    case jobStarted(JobView)
    case jobFinished(JobView)
    case jobs(clientRef: String?, [JobView])
    case jobOutput(clientRef: String?, JobOutput)
    /// The control service's status; this app does not show it.
    case control(clientRef: String?)
    case notice(level: Level, message: String, tab: String?)
    /// One tab's mode, or every tab's of `workspace` when `tab` is nil.
    case mode(Mode, tab: String?, workspace: String?)
    case clients(Int)
    /// A tunnel started or stopped, a device paired or was revoked.
    case remote(RemoteStatus)
    /// Keep-awake took or released its assertion, or its mode changed.
    case awake(AwakeStatus)
    /// The user's configuration changed: a provider was added, removed or
    /// edited, a key stored, or the default model moved. Pickers take the new list.
    case models([String], defaultModel: String?)
    /// Voice settings changed or the providers' model lists were read again.
    case voice(VoiceStatus)
    /// A sentence to read aloud, played by the connection whose listener it names.
    case say(SpokenLine)
    /// The user answered an approval or sent a prompt: stop these sentences
    /// at once if one is playing, and drop the rest from the queue.
    case sayCancelled(ids: [String])
    /// The reply to `voice_listen`: this connection's listener id, nil after `on: false`.
    case voiceListening(clientRef: String?, listener: Int?)
    case reloaded
    /// The connection fell behind; send `snapshot` to resync.
    case lagged(skipped: Int)
    case shutdown(reason: String)
    /// A message this app does not know, from a newer daemon.
    case unknown(type: String, clientRef: String?)

    /// The reference of the request this message answers.
    public var clientRef: String? {
        switch self {
        case .snapshot(let snapshot): snapshot.clientRef
        case .ack(let ack): ack.clientRef
        case .error(let ref, _), .pong(let ref), .launched(let ref, _, _, _), .workspaces(let ref, _),
             .workspaceOpened(let ref, _), .jobs(let ref, _), .jobOutput(let ref, _), .control(let ref),
             .voiceListening(let ref, _), .unknown(_, let ref):
            ref
        default:
            nil
        }
    }

    public static func decode(_ data: Data) throws -> ServerMessage {
        try JSONDecoder.malac().decode(ServerMessage.self, from: data)
    }
}

extension ServerMessage: Decodable {
    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        let type: String = try c.value("type")
        let ref: String? = c.optional("client_ref")
        switch type {
        case "snapshot":
            self = .snapshot(try Snapshot(from: decoder))
        case "event":
            self = .event(tab: c.optional("tab"), workspace: c.optional("workspace"), envelope: try c.value("envelope"))
        case "ack":
            self = .ack(Ack(
                clientRef: ref,
                agent: c.optional("agent"),
                tab: c.optional("tab"),
                workspace: c.optional("workspace"),
                attachments: c.value("attachments", default: [])
            ))
        case "error":
            self = .error(clientRef: ref, message: c.value("message", default: "Unknown error"))
        case "pong":
            self = .pong(clientRef: ref)
        case "launched":
            self = .launched(clientRef: ref, workspace: try c.value("workspace"), tab: c.optional("tab"), agent: c.optional("agent"))
        case "workspaces":
            self = .workspaces(clientRef: ref, c.value("workspaces", default: []))
        case "workspace_opened":
            self = .workspaceOpened(clientRef: ref, try c.value("workspace"))
        case "workspace_created":
            self = .workspaceCreated(try c.value("workspace"))
        case "workspace_updated":
            self = .workspaceUpdated(try c.value("workspace"))
        case "workspace_deleted":
            self = .workspaceDeleted(try c.value("workspace"))
        case "tab_created":
            self = .tabCreated(workspace: try c.value("workspace"), index: c.value("index", default: Int.max), tab: try c.value("tab"))
        case "tab_updated":
            self = .tabUpdated(workspace: try c.value("workspace"), tab: try c.value("tab"))
        case "tab_closed":
            self = .tabClosed(workspace: try c.value("workspace"), tab: try c.value("tab"))
        case "tabs_reordered":
            self = .tabsReordered(workspace: try c.value("workspace"), tabs: c.value("tabs", default: []))
        case "job_started":
            self = .jobStarted(try c.value("job"))
        case "job_finished":
            self = .jobFinished(try c.value("job"))
        case "jobs":
            self = .jobs(clientRef: ref, c.value("jobs", default: []))
        case "job_output":
            self = .jobOutput(clientRef: ref, try c.value("output"))
        case "control":
            self = .control(clientRef: ref)
        case "notice":
            self = .notice(level: c.value("level", default: .info), message: c.value("message", default: ""), tab: c.optional("tab"))
        case "mode":
            self = .mode(c.value("mode", default: .ask), tab: c.optional("tab"), workspace: c.optional("workspace"))
        case "clients":
            self = .clients(c.value("clients", default: 0))
        case "remote":
            self = .remote(try c.value("remote"))
        case "awake":
            self = .awake(try c.value("awake"))
        case "models":
            self = .models(c.value("models", default: []), defaultModel: c.optional("default_model"))
        case "voice":
            self = .voice(try c.value("voice"))
        case "say":
            self = .say(SpokenLine(
                id: try c.value("id"),
                text: c.value("text", default: ""),
                reason: c.value("reason", default: ""),
                tab: c.optional("tab"),
                agent: c.optional("agent"),
                listener: try c.value("listener")
            ))
        case "say_cancelled":
            self = .sayCancelled(ids: c.value("ids", default: []))
        case "voice_listening":
            self = .voiceListening(clientRef: ref, listener: c.optional("listener"))
        case "reloaded":
            self = .reloaded
        case "lagged":
            self = .lagged(skipped: c.value("skipped", default: 0))
        case "shutdown":
            self = .shutdown(reason: c.value("reason", default: ""))
        default:
            self = .unknown(type: type, clientRef: ref)
        }
    }
}
