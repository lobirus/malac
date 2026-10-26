import Foundation

/// What the app knows about one agent, from snapshots and its events.
public struct AgentState: Identifiable, Hashable, Sendable {
    public var id: String
    public var label = "agent"
    /// The first prompt, which names a conversation.
    public var title = ""
    public var parent: String?
    public var tab: String?
    public var model = ""
    public var cwd = ""
    public var created: Date?
    public var status = AgentStatus.idle
    /// What it does right now, e.g. the running tool.
    public var activity = ""
    public var usage = Usage.zero
    public var contextTokens = 0
    public var report: String?
    public var todos: [Todo] = []
    public var readOnly = false
    /// Loaded in the daemon, as opposed to a saved session.
    public var live = false
    /// Running tool calls, in start order.
    public var running: [RunningTool] = []
    public var turnStarted: Date?

    public init(id: String) {
        self.id = id
    }

    /// The title for top-level agents, the label for subagents.
    public var displayName: String { parent == nil && !title.isEmpty ? title : label }

    public var isTopLevel: Bool { parent == nil }
}

/// A tool call in progress, as "tool summary".
public struct RunningTool: Hashable, Sendable {
    public var callId: String
    public var text: String
}

/// The status shown for an agent, a tab or a workspace.
public enum Phase: Hashable, Sendable {
    case awaitingApproval, running, done, failed, stopped, idle

    public init(_ status: AgentStatus) {
        switch status {
        case .running: self = .running
        case .completed: self = .done
        case .failed: self = .failed
        case .cancelled: self = .stopped
        case .idle: self = .idle
        }
    }

    public var label: String {
        switch self {
        case .awaitingApproval: "Needs approval"
        case .running: "Running"
        case .done: "Done"
        case .failed: "Failed"
        case .stopped: "Stopped"
        case .idle: "Idle"
        }
    }
}
