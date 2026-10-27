import Foundation

public struct Usage: Hashable, Sendable, Decodable {
    public var inputTokens: Int
    public var outputTokens: Int
    public var cacheReadTokens: Int
    public var cacheWriteTokens: Int
    /// In USD, when the provider reports it.
    public var cost: Double

    public static let zero = Usage(inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, cost: 0)

    public init(inputTokens: Int, outputTokens: Int, cacheReadTokens: Int = 0, cacheWriteTokens: Int = 0, cost: Double = 0) {
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cacheReadTokens = cacheReadTokens
        self.cacheWriteTokens = cacheWriteTokens
        self.cost = cost
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        self.init(
            inputTokens: c.value("input_tokens", default: 0),
            outputTokens: c.value("output_tokens", default: 0),
            cacheReadTokens: c.value("cache_read_tokens", default: 0),
            cacheWriteTokens: c.value("cache_write_tokens", default: 0),
            cost: c.value("cost", default: 0)
        )
    }

    public static func + (a: Usage, b: Usage) -> Usage {
        Usage(
            inputTokens: a.inputTokens + b.inputTokens,
            outputTokens: a.outputTokens + b.outputTokens,
            cacheReadTokens: a.cacheReadTokens + b.cacheReadTokens,
            cacheWriteTokens: a.cacheWriteTokens + b.cacheWriteTokens,
            cost: a.cost + b.cost
        )
    }
}

/// The daemon's view of an agent: the runtime snapshot plus `live` and, for
/// saved sessions, `title`.
public struct AgentView: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var label: String
    public var parent: String?
    public var depth: Int
    public var readOnly: Bool
    public var cwd: String
    public var created: Date?
    public var model: String
    public var status: AgentStatus
    public var activity: String
    public var usage: Usage
    public var report: String?
    public var live: Bool
    /// The first prompt, for saved sessions.
    public var title: String

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        label = c.value("label", default: "agent")
        parent = c.optional("parent")
        depth = c.value("depth", default: 0)
        readOnly = c.value("read_only", default: false)
        cwd = c.value("cwd", default: "")
        created = c.optional("created")
        model = c.value("model", default: "")
        status = c.value("status", default: .idle)
        activity = c.value("activity", default: "")
        usage = c.value("usage", default: .zero)
        report = c.optional("report")
        live = c.value("live", default: false)
        title = c.value("title", default: "")
    }
}

/// What a tab was opened with.
public struct TabOptions: Hashable, Sendable, Decodable {
    public var model: String?
    public var mode: Mode?
    public var profile: String?
    public var noControl: Bool

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        model = c.optional("model")
        mode = c.optional("mode")
        profile = c.optional("profile")
        noControl = c.value("no_control", default: false)
    }
}

/// A project: a directory with its own runtime and the top-level agents working in it.
public struct WorkspaceTab: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var name: String
    public var cwd: String
    /// Top-level agents, in order.
    public var agents: [String]
    public var activeAgent: String?
    /// The permission mode in effect.
    public var mode: Mode
    public var options: TabOptions?
    /// Tabs load their runtime on first use.
    public var loaded: Bool
    /// The top-level agents, live or saved, and the live subagents.
    public var snapshots: [AgentView]

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        name = c.value("name", default: "")
        cwd = c.value("cwd", default: "")
        agents = c.value("agents", default: [])
        activeAgent = c.optional("active_agent")
        mode = c.value("mode", default: .ask)
        options = c.optional("options")
        loaded = c.value("loaded", default: false)
        snapshots = c.value("snapshots", default: [])
    }
}

public struct WorkspaceSummary: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var name: String
    public var tabs: Int
    public var activeTab: String?
    /// Agents of this workspace working right now.
    public var running: Int
    public var created: Date?
    public var updated: Date?

    public init(id: String, name: String, tabs: Int, activeTab: String?, running: Int, created: Date?, updated: Date?) {
        self.id = id
        self.name = name
        self.tabs = tabs
        self.activeTab = activeTab
        self.running = running
        self.created = created
        self.updated = updated
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        self.init(
            id: try c.value("id"),
            name: c.value("name", default: ""),
            tabs: c.value("tabs", default: 0),
            activeTab: c.optional("active_tab"),
            running: c.value("running", default: 0),
            created: c.optional("created"),
            updated: c.optional("updated")
        )
    }
}

/// A workspace with its tabs: what a connection has open.
public struct WorkspaceView: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var name: String
    public var activeTab: String?
    public var created: Date?
    public var updated: Date?
    public var tabs: [WorkspaceTab]

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        name = c.value("name", default: "")
        activeTab = c.optional("active_tab")
        created = c.optional("created")
        updated = c.optional("updated")
        tabs = c.value("tabs", default: [])
    }

    public var summary: WorkspaceSummary {
        let running = tabs.flatMap(\.snapshots).filter { $0.status == .running }.count
        return WorkspaceSummary(id: id, name: name, tabs: tabs.count, activeTab: activeTab, running: running, created: created, updated: updated)
    }
}

/// A tool call waiting for approval.
public struct PendingApproval: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    /// Matches the `tool_use` block and the events' `call_id`.
    public var callId: String
    public var agent: String
    public var agentLabel: String
    public var tool: String
    public var summary: String
    public var input: JSONValue
    public var tab: String?

    public init(id: String, callId: String, agent: String, agentLabel: String, tool: String, summary: String, input: JSONValue, tab: String?) {
        self.id = id
        self.callId = callId
        self.agent = agent
        self.agentLabel = agentLabel
        self.tool = tool
        self.summary = summary
        self.input = input
        self.tab = tab
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        self.init(
            id: try c.value("id"),
            callId: c.value("call_id", default: ""),
            agent: c.value("agent", default: ""),
            agentLabel: c.value("agent_label", default: "agent"),
            tool: c.value("tool", default: ""),
            summary: c.value("summary", default: ""),
            input: c.value("input", default: .null),
            tab: c.optional("tab")
        )
    }
}

/// A background process an agent started.
public struct JobView: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var command: String
    public var cwd: String
    public var pid: Int?
    public var log: String
    public var started: Date?
    public var finished: Date?
    public var exitCode: Int?
    public var status: JobStatus
    /// The agent that started it.
    public var agent: String?
    /// The tab of that agent, when known.
    public var tab: String?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        command = c.value("command", default: "")
        cwd = c.value("cwd", default: "")
        pid = c.optional("pid")
        log = c.value("log", default: "")
        started = c.optional("started")
        finished = c.optional("finished")
        exitCode = c.optional("exit_code")
        status = c.value("status", default: .unknown)
        agent = c.optional("agent")
        tab = c.optional("tab")
    }

    public var isRunning: Bool { status == .running }
}

/// The last lines of a job's log.
public struct JobOutput: Hashable, Sendable, Decodable {
    public var job: JobView
    public var text: String
    /// Size of the whole log in bytes.
    public var size: Int
    /// Whether `text` starts after the beginning of the log.
    public var truncated: Bool

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        job = try c.value("job")
        text = c.value("text", default: "")
        size = c.value("size", default: 0)
        truncated = c.value("truncated", default: false)
    }
}

public struct DaemonStatus: Hashable, Sendable, Decodable {
    public var version: String
    public var pid: Int
    public var started: Date?
    public var versionMismatch: Bool
    /// Connected WebSocket clients, this one included.
    public var clients: Int
    /// A terminal session's own server rather than the daemon: workspace and tab commands are refused.
    public var attached: Bool
    public var remote: Bool
    /// So paths can be shown as `~/…`.
    public var userHome: String?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        version = c.value("version", default: "")
        pid = c.value("pid", default: 0)
        started = c.optional("started")
        versionMismatch = c.value("version_mismatch", default: false)
        clients = c.value("clients", default: 0)
        attached = c.value("attached", default: false)
        remote = c.value("remote", default: false)
        userHome = c.optional("user_home")
    }
}

public struct UpdateStatus: Hashable, Sendable, Decodable {
    public var current: String
    public var latest: String?
    /// A newer release exists and is not installed yet.
    public var available: Bool
    /// A newer version is installed; restarting the daemon picks it up.
    public var installed: String?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        current = c.value("current", default: "")
        latest = c.optional("latest")
        available = c.value("available", default: false)
        installed = c.optional("installed")
    }
}

/// The first message on every connection, and the reply to `snapshot`.
public struct Snapshot: Sendable, Decodable {
    public var clientRef: String?
    public var daemon: DaemonStatus
    public var workspaces: [WorkspaceSummary]
    /// The connection's workspace.
    public var workspace: WorkspaceView?
    /// Requests waiting for an answer, from every tab of every workspace.
    public var pending: [PendingApproval]
    public var jobs: [JobView]
    public var update: UpdateStatus?
    /// Every configured `provider/model`.
    public var models: [String]
    public var defaultModel: String?
    public var remote: RemoteStatus?
    public var awake: AwakeStatus?
    /// Dictation, speech and talk mode; nil from daemons without voice.
    public var voice: VoiceStatus?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        clientRef = c.optional("client_ref")
        daemon = try c.value("daemon")
        workspaces = c.value("workspaces", default: [])
        workspace = c.optional("workspace")
        pending = c.value("pending", default: [])
        jobs = c.value("jobs", default: [])
        update = c.optional("update")
        models = c.value("models", default: [])
        defaultModel = c.optional("default_model")
        remote = c.optional("remote")
        awake = c.optional("awake")
        voice = c.optional("voice")
    }
}

/// What the daemon made of a prompt attachment.
public struct AttachmentInfo: Hashable, Sendable, Decodable {
    public var path: String
    /// `text`, `image`, `directory` or `binary`.
    public var kind: String
    public var size: Int
    public var note: String?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        path = c.value("path", default: "")
        kind = c.value("kind", default: "")
        size = c.value("size", default: 0)
        note = c.optional("note")
    }
}

public struct Todo: Hashable, Sendable, Decodable {
    public var content: String
    public var status: TodoStatus

    public init(content: String, status: TodoStatus) {
        self.content = content
        self.status = status
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        content = c.value("content", default: "")
        status = c.value("status", default: .pending)
    }
}

/// A saved session, from `GET /api/sessions`.
public struct SessionSummary: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var label: String
    public var parent: String?
    public var cwd: String
    public var model: String
    public var created: Date?
    /// The first prompt's typed text.
    public var title: String
    public var updated: Date?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        label = c.value("label", default: "agent")
        parent = c.optional("parent")
        cwd = c.value("cwd", default: "")
        model = c.value("model", default: "")
        created = c.optional("created")
        title = c.value("title", default: "")
        updated = c.optional("updated")
    }
}

/// A saved transcript, from `GET /api/sessions/{id}`.
public struct SessionTranscript: Sendable, Decodable {
    public var history: [Message]
    public var usage: Usage?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        history = c.value("history", default: [])
        usage = c.optional("usage")
    }
}
