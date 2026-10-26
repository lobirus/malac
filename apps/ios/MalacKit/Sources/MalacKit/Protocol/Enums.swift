public enum AgentStatus: String, LenientEnum {
    case idle, running, completed, failed, cancelled
    public static let fallback = AgentStatus.idle
}

/// How a turn ended.
public enum TurnStatus: String, LenientEnum {
    case completed, cancelled, failed
    public static let fallback = TurnStatus.failed

    public var agentStatus: AgentStatus {
        switch self {
        case .completed: .completed
        case .cancelled: .cancelled
        case .failed: .failed
        }
    }
}

/// A tab's permission mode.
public enum Mode: String, LenientEnum, CaseIterable, Identifiable {
    /// Confirm edits and commands.
    case ask
    /// Edit files freely, confirm commands.
    case edits
    /// Never ask. Deny rules still apply.
    case autopilot
    public static let fallback = Mode.ask

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .ask: "Ask"
        case .edits: "Edits"
        case .autopilot: "Autopilot"
        }
    }

    public var explanation: String {
        switch self {
        case .ask: "Confirm edits and commands"
        case .edits: "Edit files freely, confirm commands"
        case .autopilot: "Never ask; deny rules still apply"
        }
    }
}

/// An answer to a permission request.
public enum Decision: String, Codable, Sendable, CaseIterable {
    case allow
    /// Allow this and similar calls for the rest of the session.
    case allowSession = "allow_session"
    case deny
}

public enum JobStatus: String, LenientEnum {
    case running, exited, killed, unknown
    public static let fallback = JobStatus.unknown
}

public enum Level: String, LenientEnum {
    case info, warning, error
    public static let fallback = Level.info
}

public enum TodoStatus: String, LenientEnum {
    case pending
    case inProgress = "in_progress"
    case completed
    public static let fallback = TodoStatus.pending
}

public enum Role: String, LenientEnum {
    case user, assistant
    public static let fallback = Role.assistant
}
