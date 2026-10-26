import Foundation

/// An image sent with a prompt.
public struct ImageInput: Hashable, Sendable, Encodable {
    public var mediaType: String
    /// Base64.
    public var data: String
    /// Shows the image as an attachment of this name, e.g. a pasted screenshot.
    public var name: String?

    public init(mediaType: String, data: String, name: String? = nil) {
        self.mediaType = mediaType
        self.data = data
        self.name = name
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put(mediaType, "media_type")
        try c.put(data, "data")
        try c.put(ifPresent: name, "name")
    }

    public var block: ImageBlock { ImageBlock(mediaType: mediaType, data: data) }
}

/// A client command. Commands are sent as JSON objects whose `type` names them.
public enum Command: Hashable, Sendable {
    /// Prompts `agent`, or starts a new top-level agent in `tab` and prompts it.
    case prompt(text: String, agent: String?, tab: String?, images: [ImageInput] = [], model: String? = nil)
    case agentStart(tab: String, model: String?)
    case agentOpen(agent: String, tab: String?)
    case agentActivate(tab: String, agent: String)
    /// Cancels the agent's turn and its subagents'.
    case cancel(agent: String)
    case approve(requestId: String, decision: Decision)
    /// One tab's mode, or every tab's of the connection's workspace.
    case mode(Mode, tab: String?)
    case model(agent: String, model: String)
    case compact(agent: String)
    /// The snapshot again; with a workspace, switches the connection to it.
    case snapshot(workspace: String?)
    case workspaceList
    case tabActivate(tab: String)
    case jobList
    case jobOutput(job: String, tail: Int?)
    case jobKill(job: String)
    /// Until the daemon restarts.
    case keepAwake(KeepAwake)
    case ping

    public var type: String {
        switch self {
        case .prompt: "prompt"
        case .agentStart: "agent_start"
        case .agentOpen: "agent_open"
        case .agentActivate: "agent_activate"
        case .cancel: "cancel"
        case .approve: "approve"
        case .mode: "mode"
        case .model: "model"
        case .compact: "compact"
        case .snapshot: "snapshot"
        case .workspaceList: "workspace_list"
        case .tabActivate: "tab_activate"
        case .jobList: "job_list"
        case .jobOutput: "job_output"
        case .jobKill: "job_kill"
        case .keepAwake: "keep_awake"
        case .ping: "ping"
        }
    }

    /// The JSON text of this command, with `client_ref` when the reply is awaited.
    public func encoded(clientRef: String? = nil) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(Request(command: self, clientRef: clientRef))
    }
}

private struct Request: Encodable {
    let command: Command
    let clientRef: String?

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put(command.type, "type")
        try c.put(ifPresent: clientRef, "client_ref")
        switch command {
        case let .prompt(text, agent, tab, images, model):
            try c.put(text, "text")
            try c.put(ifPresent: agent, "agent")
            try c.put(ifPresent: tab, "tab")
            if !images.isEmpty { try c.put(images, "images") }
            try c.put(ifPresent: model, "model")
        case let .agentStart(tab, model):
            try c.put(tab, "tab")
            try c.put(ifPresent: model, "model")
        case let .agentOpen(agent, tab):
            try c.put(agent, "agent")
            try c.put(ifPresent: tab, "tab")
        case let .agentActivate(tab, agent):
            try c.put(tab, "tab")
            try c.put(agent, "agent")
        case let .cancel(agent), let .compact(agent):
            try c.put(agent, "agent")
        case let .approve(requestId, decision):
            try c.put(requestId, "request_id")
            try c.put(decision, "decision")
        case let .mode(mode, tab):
            try c.put(mode, "mode")
            try c.put(ifPresent: tab, "tab")
        case let .model(agent, model):
            try c.put(agent, "agent")
            try c.put(model, "model")
        case let .snapshot(workspace):
            try c.put(ifPresent: workspace, "workspace")
        case let .tabActivate(tab):
            try c.put(tab, "tab")
        case let .jobOutput(job, tail):
            try c.put(job, "job")
            try c.put(ifPresent: tail, "tail")
        case let .jobKill(job):
            try c.put(job, "job")
        case let .keepAwake(mode):
            try c.put(mode, "mode")
        case .workspaceList, .jobList, .ping:
            break
        }
    }
}
