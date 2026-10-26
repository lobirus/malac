import Foundation

/// An agent event with the agent's id, a sequence number that increases
/// across every tab, and a timestamp.
public struct Envelope: Hashable, Sendable {
    public var agent: String
    public var seq: Int
    public var at: Date
    public var event: AgentEvent

    public init(agent: String, seq: Int, at: Date, event: AgentEvent) {
        self.agent = agent
        self.seq = seq
        self.at = at
        self.event = event
    }
}

/// The runtime's events (`crates/malac-core/src/event.rs`).
public enum AgentEvent: Hashable, Sendable {
    case agentStarted(label: String, parent: String?, model: String, cwd: String)
    case modelChanged(model: String)
    case userMessage(text: String)
    case textDelta(String)
    case thinkingDelta(String)
    case toolCallStreaming(tool: String)
    case assistantMessage(Message)
    case toolStarted(callId: String, tool: String, summary: String, input: JSONValue)
    case toolFinished(callId: String, tool: String, output: String, isError: Bool, millis: Int)
    case permissionRequested(requestId: String, callId: String, tool: String, summary: String, input: JSONValue)
    case permissionResolved(requestId: String, allowed: Bool)
    case todos([Todo])
    case usage(turn: Usage, total: Usage, contextTokens: Int)
    case compacted(summary: String)
    case notice(level: Level, message: String)
    case turnFinished(status: TurnStatus, text: String)
    case agentFinished(status: TurnStatus, report: String)
    /// Published with an empty agent id when a newer malac is available or installed.
    case update(UpdateStatus)
    case unknown(type: String)
}

extension Envelope: Decodable {
    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        agent = c.value("agent", default: "")
        seq = try c.value("seq")
        at = c.value("at", default: Date(timeIntervalSince1970: 0))
        let type: String = try c.value("type")
        switch type {
        case "agent_started":
            event = .agentStarted(
                label: c.value("label", default: "agent"),
                parent: c.optional("parent"),
                model: c.value("model", default: ""),
                cwd: c.value("cwd", default: "")
            )
        case "model_changed":
            event = .modelChanged(model: c.value("model", default: ""))
        case "user_message":
            event = .userMessage(text: c.value("text", default: ""))
        case "text_delta":
            event = .textDelta(c.value("text", default: ""))
        case "thinking_delta":
            event = .thinkingDelta(c.value("text", default: ""))
        case "tool_call_streaming":
            event = .toolCallStreaming(tool: c.value("tool", default: ""))
        case "assistant_message":
            event = .assistantMessage(try c.value("message"))
        case "tool_started":
            event = .toolStarted(
                callId: c.value("call_id", default: ""),
                tool: c.value("tool", default: ""),
                summary: c.value("summary", default: ""),
                input: c.value("input", default: .null)
            )
        case "tool_finished":
            event = .toolFinished(
                callId: c.value("call_id", default: ""),
                tool: c.value("tool", default: ""),
                output: c.value("output", default: ""),
                isError: c.value("is_error", default: false),
                millis: c.value("millis", default: 0)
            )
        case "permission_requested":
            event = .permissionRequested(
                requestId: try c.value("request_id"),
                callId: c.value("call_id", default: ""),
                tool: c.value("tool", default: ""),
                summary: c.value("summary", default: ""),
                input: c.value("input", default: .null)
            )
        case "permission_resolved":
            event = .permissionResolved(requestId: try c.value("request_id"), allowed: c.value("allowed", default: false))
        case "todos":
            event = .todos(c.value("items", default: []))
        case "usage":
            event = .usage(
                turn: c.value("turn", default: .zero),
                total: c.value("total", default: .zero),
                contextTokens: c.value("context_tokens", default: 0)
            )
        case "compacted":
            event = .compacted(summary: c.value("summary", default: ""))
        case "notice":
            event = .notice(level: c.value("level", default: .info), message: c.value("message", default: ""))
        case "turn_finished":
            event = .turnFinished(status: c.value("status", default: .failed), text: c.value("text", default: ""))
        case "agent_finished":
            event = .agentFinished(status: c.value("status", default: .failed), report: c.value("report", default: ""))
        case "update":
            event = .update(try c.value("status"))
        default:
            event = .unknown(type: type)
        }
    }
}
