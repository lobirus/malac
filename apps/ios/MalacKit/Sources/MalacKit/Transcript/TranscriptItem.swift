import Foundation

/// One row of a transcript. `id` is stable across updates, for list diffing.
public enum TranscriptItem: Identifiable, Hashable, Sendable {
    case user(UserMessage)
    case assistant(AssistantText)
    case thinking(id: String, text: String)
    /// Memories attached to a prompt by vector recall.
    case recalled(id: String, text: String)
    case tool(ToolCall)
    case subagent(SubagentCall)
    case plan(Plan)
    case compacted(id: String, summary: String)
    case modelChanged(id: String, model: String)
    case notice(id: String, level: Level, message: String)
    case turnEnd(TurnSummary)

    public var id: String {
        switch self {
        case .user(let item): item.id
        case .assistant(let item): item.id
        case .thinking(let id, _), .recalled(let id, _), .compacted(let id, _), .modelChanged(let id, _), .notice(let id, _, _): id
        case .tool(let call): call.key
        case .subagent(let call): call.key
        case .plan(let plan): plan.id
        case .turnEnd(let summary): summary.id
        }
    }
}

public struct UserMessage: Hashable, Sendable {
    public var id: String
    public var text: String
    public var images: [ImageBlock] = []
    public var attachments: [Attachment] = []
    public var at: Date?
}

/// A file attached to a prompt; `body` and `images` are known only from the saved history.
public struct Attachment: Hashable, Sendable {
    public var path: String
    public var body: String?
    public var images: [ImageBlock] = []
}

public struct AssistantText: Hashable, Sendable {
    public var id: String
    public var markdown: String
    public var at: Date?
    /// The turn ended while this text was streaming.
    public var interrupted = false
}

public enum ToolState: Hashable, Sendable {
    case pending, running, awaitingApproval, done, failed, denied, skipped
}

public struct ToolCall: Hashable, Sendable {
    /// The row id.
    public var key: String
    /// The call id, shared with `tool_started`, `tool_finished` and permission requests.
    public var id: String
    public var name: String
    public var input: JSONValue
    public var state = ToolState.pending
    /// The open permission request, while `state` is `awaitingApproval`.
    public var requestId: String?
    public var output: String?
    public var millis: Int?
    public var images: [ImageBlock] = []
    public var at: Date?

    public var isError: Bool { state == .failed || state == .denied }
    public var isFinished: Bool { [.done, .failed, .denied].contains(state) }
}

/// An `agent_spawn` call and the subagent it started.
public struct SubagentCall: Hashable, Sendable {
    public var key: String
    public var id: String
    public var label: String
    public var task: String
    /// The agent type, when the model asked for one other than the label.
    public var agentType: String?
    public var readOnly: Bool
    public var child: String?
    public var started = false
    public var done = false
    public var isError = false
    public var output = ""
    public var millis: Int?
    /// The turn ended before the subagent started.
    public var abandoned = false

    /// The live child's status when it is loaded, else what the call says.
    public func status(child agent: AgentState?) -> AgentStatus {
        if let agent, agent.live { return agent.status }
        if !done { return started ? .running : .idle }
        if !isError { return .completed }
        return output.hasPrefix("cancelled") ? .cancelled : .failed
    }

    public func report(child agent: AgentState?) -> String {
        if let report = agent?.report, !report.isEmpty { return report }
        return done ? ToolPresentation.spawnReport(output) : ""
    }
}

public struct Plan: Hashable, Sendable {
    public var id: String
    public var items: [Todo]
    /// A later plan replaced this one.
    public var superseded = false
    public var error: String?

    public var completed: Int { items.filter { $0.status == .completed }.count }
}

public struct TurnSummary: Hashable, Sendable {
    public var id: String
    public var status: TurnStatus
    public var at: Date
    public var milliseconds: Int?
    public var usage: Usage?
    public var contextTokens = 0
}

/// What the agent is streaming right now, shown after the items.
public struct Draft: Hashable, Sendable {
    public var text = ""
    public var thinking = ""
    public var preparingTool: String?
}
