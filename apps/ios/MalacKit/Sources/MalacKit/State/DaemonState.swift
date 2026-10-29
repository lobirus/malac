import Foundation

/// An entry of an agent's event log, replayed by a transcript after its saved history.
public enum LogEntry: Hashable, Sendable {
    case event(Envelope)
    /// A subagent of this agent started; links it to its `agent_spawn` call.
    case childStarted(seq: Int, child: String, label: String, at: Date)

    public var seq: Int {
        switch self {
        case .event(let envelope): envelope.seq
        case .childStarted(let seq, _, _, _): seq
        }
    }
}

/// What the app does in response to a message, beyond updating the state.
public enum Effect: Hashable, Sendable {
    /// Send `snapshot`, for this workspace or the connection's.
    case resync(workspace: String?)
    case notice(Level, String)
    case approvalRequested(PendingApproval)
    /// Answered, here or by another client.
    case approvalResolved(requestId: String)
    /// A top-level agent finished a turn.
    case turnFinished(AgentState, TurnStatus, text: String)
    /// A sentence for this connection to read aloud.
    case speak(SpokenLine)
    /// Stop these sentences: the one playing at once, the queued ones before they start.
    case stopSpeaking(ids: Set<String>)
}

/// The daemon as one connection sees it: the first message is a snapshot of
/// the connection's workspace, then broadcasts keep it current. Agents of
/// other workspaces are known from their events; tabs only for the workspace
/// this connection has open. Mirrors `web/store.js`.
public struct DaemonState: Sendable {
    public struct TabRef: Hashable, Sendable {
        public var name: String
        public var workspace: String?
    }

    public static let logLimit = 2000

    public private(set) var daemon: DaemonStatus?
    public private(set) var workspaces: [WorkspaceSummary] = []
    public private(set) var workspace: WorkspaceView?
    public private(set) var agents: [String: AgentState] = [:]
    /// Requests waiting for an answer, from every workspace, oldest first.
    public private(set) var pending: [PendingApproval] = []
    public private(set) var jobs: [String: JobView] = [:]
    public private(set) var models: [String] = []
    public private(set) var defaultModel: String?
    public private(set) var update: UpdateStatus?
    public private(set) var remote: RemoteStatus?
    public private(set) var awake: AwakeStatus?
    /// Dictation, speech and talk mode; nil from daemons without voice.
    public private(set) var voice: VoiceStatus?
    /// This connection's listener id while it plays speech, from `voice_listening`.
    public private(set) var listener: Int?
    /// Set by `shutdown`, cleared by the next snapshot.
    public private(set) var shutdownReason: String?
    /// Tab names and workspaces seen so far, to label approvals from other workspaces.
    public private(set) var tabs: [String: TabRef] = [:]
    public private(set) var logs: [String: [LogEntry]] = [:]
    public private(set) var lastSeq = -1
    /// Increases with every snapshot; transcripts reload their history when it changes.
    public private(set) var generation = 0

    public init() {}

    public var isLoaded: Bool { daemon != nil }

    // MARK: Queries

    public func tab(_ id: String?) -> WorkspaceTab? {
        workspace?.tabs.first { $0.id == id }
    }

    public func log(_ agent: String) -> [LogEntry] { logs[agent] ?? [] }

    public func pending(for agent: String) -> [PendingApproval] { pending.filter { $0.agent == agent } }

    /// Whether this connection saw the agent being created, so its log holds
    /// the whole conversation and there is no saved history to fetch. A
    /// resumed session starts again too; its UUIDv7 id says it is older.
    public func sawCreation(of agent: String) -> Bool {
        guard case .event(let first)? = logs[agent]?.first, case .agentStarted = first.event,
              let millis = Int(agent.replacingOccurrences(of: "-", with: "").prefix(12), radix: 16) else { return false }
        return abs(first.at.timeIntervalSince1970 - Double(millis) / 1000) < 5
    }

    public func phase(of agent: AgentState) -> Phase {
        if pending.contains(where: { $0.agent == agent.id }) { return .awaitingApproval }
        return agent.live ? Phase(agent.status) : .idle
    }

    /// The top-level agents of `tab`, in order.
    public func agents(of tab: WorkspaceTab) -> [AgentState] { tab.agents.compactMap { agents[$0] } }

    public func children(of agent: String) -> [AgentState] {
        agents.values.filter { $0.parent == agent }.sorted { ($0.created ?? .distantPast) < ($1.created ?? .distantPast) }
    }

    /// The tab that shows `agent`: its own, or an ancestor's.
    public func tab(of agent: String) -> WorkspaceTab? {
        let tabs = workspace?.tabs ?? []
        if let direct = tabs.first(where: { $0.agents.contains(agent) || $0.snapshots.contains { $0.id == agent } }) {
            return direct
        }
        var current = agents[agent]
        if let id = current?.tab, let own = tab(id) { return own }
        for _ in 0 ..< 32 {
            guard let parent = current?.parent else { return nil }
            if let found = tabs.first(where: { $0.agents.contains(parent) }) { return found }
            current = agents[parent]
        }
        return nil
    }

    /// Waiting, running, failed or done, like the web UI's tab badges.
    public func phase(of tab: WorkspaceTab) -> Phase {
        let ids = Set(tab.agents + tab.snapshots.map(\.id))
        let members = agents.values.filter { ids.contains($0.id) || ($0.tab == tab.id && $0.live) }
        if pending.contains(where: { $0.tab == tab.id || ids.contains($0.agent) }) { return .awaitingApproval }
        if members.contains(where: { $0.live && $0.status == .running }) { return .running }
        if members.contains(where: { $0.status == .failed }) { return .failed }
        if members.contains(where: { $0.status == .completed }) { return .done }
        return .idle
    }

    public func phase(of summary: WorkspaceSummary) -> Phase {
        if pending.contains(where: { approval in approval.tab.flatMap { tabs[$0]?.workspace } == summary.id }) {
            return .awaitingApproval
        }
        return summary.running > 0 ? .running : .idle
    }

    /// "tab · workspace" for an approval, as far as it is known.
    public func place(of approval: PendingApproval) -> String {
        let ref = approval.tab.flatMap { tabs[$0] }
        let workspaceName = ref?.workspace.flatMap { id in workspaces.first { $0.id == id }?.name }
        return [ref?.name, workspaceName].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
    }

    /// The workspace an approval's tab belongs to, when known.
    public func workspace(of approval: PendingApproval) -> String? {
        approval.tab.flatMap { tabs[$0]?.workspace }
    }

    // MARK: Local changes

    /// Drops a request, e.g. after the daemon said it was already answered.
    public mutating func removeRequest(_ id: String) {
        pending.removeAll { $0.id == id }
    }

    /// The connection closed: a new one plays speech only after it sends `voice_listen` again.
    public mutating func forgetListener() {
        listener = nil
    }

    /// Voice settings from `GET` or `PUT /api/voice`.
    public mutating func setVoice(_ status: VoiceStatus) {
        voice = status
    }

    // MARK: Reducer

    /// Applies one message and says what else should happen.
    @discardableResult
    public mutating func apply(_ message: ServerMessage) -> [Effect] {
        var effects: [Effect] = []
        switch message {
        case .snapshot(let snapshot):
            applySnapshot(snapshot)
        case let .event(tab, workspace, envelope):
            event(tab: tab, workspace: workspace, envelope: envelope, effects: &effects)
        case .workspaceOpened(_, let view), .launched(_, let view, _, _):
            setWorkspace(view)
        case .workspaces(_, let list):
            workspaces = list
        case .workspaceCreated(let summary):
            upsert(summary)
        case .workspaceUpdated(let summary):
            upsert(summary)
            if workspace?.id == summary.id {
                workspace?.name = summary.name
                workspace?.activeTab = summary.activeTab
                workspace?.updated = summary.updated
            }
        case .workspaceDeleted(let id):
            workspaces.removeAll { $0.id == id }
            if workspace?.id == id {
                workspace = nil
                effects.append(.resync(workspace: nil))
            }
        case let .tabCreated(id, index, tab):
            resyncWithoutWorkspace(&effects)
            tabs[tab.id] = TabRef(name: tab.name, workspace: id)
            guard let view = workspace, view.id == id else { break }
            if !view.tabs.contains(where: { $0.id == tab.id }) {
                workspace?.tabs.insert(tab, at: min(max(index, 0), view.tabs.count))
            }
            applyTab(tab, workspace: id)
        case let .tabUpdated(id, tab):
            resyncWithoutWorkspace(&effects)
            tabs[tab.id] = TabRef(name: tab.name, workspace: id)
            guard workspace?.id == id else { break }
            if let index = workspace?.tabs.firstIndex(where: { $0.id == tab.id }) {
                workspace?.tabs[index] = tab
            } else {
                workspace?.tabs.append(tab)
            }
            applyTab(tab, workspace: id)
        case let .tabClosed(id, tab):
            guard workspace?.id == id else { break }
            workspace?.tabs.removeAll { $0.id == tab }
        case let .tabsReordered(id, order):
            guard workspace?.id == id, let current = workspace?.tabs else { break }
            let byId = Dictionary(current.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            workspace?.tabs = order.compactMap { byId[$0] }
        case .jobStarted(let job), .jobFinished(let job):
            jobs[job.id] = job
        case .jobs(_, let list):
            jobs = Dictionary(list.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
        case let .mode(mode, tab, workspaceId):
            // A mode for every tab names its workspace, which may be another one.
            guard let view = workspace, tab != nil || workspaceId == nil || workspaceId == view.id else { break }
            for index in view.tabs.indices where tab == nil || view.tabs[index].id == tab {
                workspace?.tabs[index].mode = mode
            }
        case .clients(let count):
            daemon?.clients = count
        case .remote(let status):
            remote = status
        case .awake(let status):
            awake = status
        case .voice(let status):
            voice = status
        case .voiceListening(_, let id):
            listener = id
        case .say(let line):
            // Every client gets every sentence; only the one it names plays it.
            if line.listener == listener { effects.append(.speak(line)) }
        case .sayCancelled(let ids):
            // Whatever the listener now, this connection may still be playing one of them.
            if !ids.isEmpty { effects.append(.stopSpeaking(ids: Set(ids))) }
        case let .notice(level, text, _):
            effects.append(.notice(level, text))
        case .lagged:
            effects.append(.resync(workspace: workspace?.id))
        case .shutdown(let reason):
            shutdownReason = reason.isEmpty ? "stopped" : reason
        case let .error(ref, text):
            if ref == nil { effects.append(.notice(.error, text)) }
        case .ack, .pong, .jobOutput, .control, .reloaded, .unknown:
            break
        }
        return effects
    }

    // MARK: Snapshot and workspaces

    private mutating func applySnapshot(_ snapshot: Snapshot) {
        daemon = snapshot.daemon
        workspaces = snapshot.workspaces
        pending = snapshot.pending
        jobs = Dictionary(snapshot.jobs.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
        update = snapshot.update ?? update
        remote = snapshot.remote
        awake = snapshot.awake
        voice = snapshot.voice
        models = snapshot.models
        defaultModel = snapshot.defaultModel
        shutdownReason = nil
        // Messages may have been missed; transcripts reload their history.
        logs = [:]
        generation += 1
        setWorkspace(snapshot.workspace)
    }

    private mutating func setWorkspace(_ view: WorkspaceView?) {
        workspace = view
        for id in agents.keys { agents[id]?.live = false }
        guard let view else { return }
        for tab in view.tabs { applyTab(tab, workspace: view.id) }
        upsert(view.summary)
    }

    private mutating func upsert(_ summary: WorkspaceSummary) {
        if let index = workspaces.firstIndex(where: { $0.id == summary.id }) {
            workspaces[index] = summary
        } else {
            workspaces.append(summary)
        }
    }

    // A command can give this connection a workspace it has not seen, as
    // `tab_create` does before any exists; only a snapshot says which.
    private func resyncWithoutWorkspace(_ effects: inout [Effect]) {
        let resyncing = effects.contains {
            if case .resync = $0 { return true }
            return false
        }
        if workspace == nil, !resyncing { effects.append(.resync(workspace: nil)) }
    }

    private mutating func applyTab(_ tab: WorkspaceTab, workspace: String) {
        tabs[tab.id] = TabRef(name: tab.name, workspace: workspace)
        for view in tab.snapshots { applyAgentView(view, tab: tab.id) }
    }

    private mutating func applyAgentView(_ view: AgentView, tab: String) {
        var agent = agents[view.id] ?? AgentState(id: view.id)
        agent.label = view.label
        agent.parent = view.parent
        agent.tab = tab
        agent.model = view.model
        agent.cwd = view.cwd
        agent.created = view.created
        agent.status = view.status
        agent.activity = view.activity
        agent.usage = view.usage
        agent.report = view.report
        agent.readOnly = view.readOnly
        agent.live = view.live
        let title = Self.cleanTitle(view.title)
        if !title.isEmpty { agent.title = title }
        agents[view.id] = agent
    }

    // MARK: Agent events

    private mutating func event(tab: String?, workspace: String?, envelope: Envelope, effects: inout [Effect]) {
        if case .update(let status) = envelope.event {
            update = status
            return
        }
        if let tab, tabs[tab]?.workspace == nil {
            tabs[tab] = TabRef(name: tabs[tab]?.name ?? "", workspace: workspace)
        }
        lastSeq = max(lastSeq, envelope.seq)
        var agent = agents[envelope.agent] ?? AgentState(id: envelope.agent)
        if let tab { agent.tab = tab }
        if case let .agentStarted(label, parent, model, cwd) = envelope.event {
            agent.label = label
            agent.parent = parent
            agent.model = model
            agent.cwd = cwd
            agent.created = envelope.at
            agent.status = .idle
            agent.live = true
            if let parent {
                if agents[parent] == nil { agents[parent] = AgentState(id: parent) }
                if agent.tab == nil { agent.tab = agents[parent]?.tab }
                record(.childStarted(seq: envelope.seq, child: agent.id, label: label, at: envelope.at), for: parent)
            }
        }
        record(.event(envelope), for: agent.id)
        track(&agent, envelope)
        agents[agent.id] = agent
        trackPermission(agent: agent, envelope: envelope, tab: tab, effects: &effects)
        if case let .turnFinished(status, text) = envelope.event, agent.isTopLevel {
            effects.append(.turnFinished(agent, status, text: text))
        }
    }

    // Consecutive deltas are merged so long streams stay cheap to replay.
    private mutating func record(_ entry: LogEntry, for agent: String) {
        var log = logs[agent] ?? []
        if case .event(let next) = entry, case .event(let last)? = log.last, let merged = Self.merge(last, next) {
            log[log.count - 1] = .event(merged)
        } else {
            if log.count >= Self.logLimit { log.removeFirst(log.count - Self.logLimit + 1) }
            log.append(entry)
        }
        logs[agent] = log
    }

    private static func merge(_ last: Envelope, _ next: Envelope) -> Envelope? {
        var merged = next
        switch (last.event, next.event) {
        case let (.textDelta(a), .textDelta(b)): merged.event = .textDelta(a + b)
        case let (.thinkingDelta(a), .thinkingDelta(b)): merged.event = .thinkingDelta(a + b)
        default: return nil
        }
        return merged
    }

    private func track(_ agent: inout AgentState, _ envelope: Envelope) {
        switch envelope.event {
        case .userMessage(let text):
            agent.status = .running
            agent.live = true
            agent.running = []
            agent.turnStarted = envelope.at
            if agent.title.isEmpty { agent.title = Format.truncate(Format.firstLine(text), 80) }
        case .modelChanged(let model):
            agent.model = model
        case let .toolStarted(callId, tool, summary, _):
            agent.running.append(RunningTool(callId: callId, text: "\(tool) \(summary)".trimmingCharacters(in: .whitespaces)))
        case let .toolFinished(callId, _, _, _, _):
            agent.running.removeAll { $0.callId == callId }
        case let .usage(_, total, contextTokens):
            agent.usage = total
            agent.contextTokens = contextTokens
        case .todos(let items):
            agent.todos = items
        case let .turnFinished(status, text):
            agent.status = status.agentStatus
            agent.report = text
            agent.running = []
            agent.turnStarted = nil
        case let .agentFinished(status, report):
            agent.status = status.agentStatus
            agent.report = report
        default:
            break
        }
        agent.activity = Self.activity(of: agent, after: envelope.event)
    }

    private static func activity(of agent: AgentState, after event: AgentEvent) -> String {
        switch event {
        case .userMessage, .thinkingDelta: "thinking"
        case .textDelta: "writing"
        case .toolCallStreaming(let tool): "preparing \(tool)"
        case .toolStarted, .toolFinished, .permissionResolved: agent.running.last?.text ?? "thinking"
        case .permissionRequested(_, _, let tool, _, _): "waiting for approval: \(tool)"
        case .turnFinished: ""
        default: agent.activity
        }
    }

    private mutating func trackPermission(agent: AgentState, envelope: Envelope, tab: String?, effects: inout [Effect]) {
        switch envelope.event {
        case let .permissionRequested(requestId, callId, tool, summary, input):
            let approval = PendingApproval(
                id: requestId, callId: callId, agent: agent.id, agentLabel: agent.label,
                tool: tool, summary: summary, input: input, tab: tab ?? agent.tab
            )
            pending.removeAll { $0.id == requestId }
            pending.append(approval)
            effects.append(.approvalRequested(approval))
        case .permissionResolved(let requestId, _):
            pending.removeAll { $0.id == requestId }
            effects.append(.approvalResolved(requestId: requestId))
        default:
            break
        }
    }

    /// A session title, unless it is attached content rather than typed text.
    static func cleanTitle(_ title: String) -> String {
        title.hasPrefix("<attachment ") ? "" : title
    }
}
