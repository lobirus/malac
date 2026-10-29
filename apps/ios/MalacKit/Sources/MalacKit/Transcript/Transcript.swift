import Foundation

/// The transcript of one agent: its saved history, then its live events, as
/// rows. Ported from `web/transcript.js`; the daemon's events reach every
/// client, so this is the view the web UI and the terminal show too.
public struct Transcript: Sendable {
    public private(set) var items: [TranscriptItem] = []
    /// What the agent is streaming right now.
    public private(set) var draft: Draft?

    private var positions: [String: Int] = [:]
    /// Open permission requests and the rows that show them.
    private var approvals: [String: String] = [:]
    private var spawnKeys: [String] = []
    private var outgoing: [(text: String, images: [ImageBlock])] = []
    private var lastPlanKey: String?
    private var turn: (started: Date, usage: Usage, context: Int)?
    private var counter = 0

    public init() {}

    /// History from `GET /api/sessions/{id}`, then the log entries that came
    /// after `cutoff`, with the open requests and known subagents linked.
    /// `openAtCutoff` are the requests that were waiting at `cutoff`: claimed
    /// before the log, so the answers that come after it find their rows.
    public init(
        history: [Message],
        decisions: [DecisionRecord] = [],
        log: [LogEntry],
        after cutoff: Int,
        openAtCutoff: [PendingApproval] = [],
        pending: [PendingApproval],
        children: [AgentState],
        sentImages: [(text: String, images: [ImageBlock])] = []
    ) {
        outgoing = sentImages
        load(history, decisions: decisions)
        claim(openAtCutoff)
        for entry in log where entry.seq > cutoff { apply(entry) }
        claim(pending)
        link(children)
    }

    // MARK: Input

    /// The saved history, with the answers to its approvals.
    public mutating func load(_ history: [Message], decisions: [DecisionRecord] = []) {
        for message in history {
            if message.role == .user { userBlocks(message.content) } else { assistantBlocks(message.content, at: nil) }
        }
        for record in decisions { decided(record) }
    }

    public mutating func apply(_ entry: LogEntry) {
        switch entry {
        case .event(let envelope): apply(envelope)
        case let .childStarted(_, child, label, _): childStarted(child, label: label)
        }
    }

    public mutating func apply(_ envelope: Envelope) {
        let at = envelope.at
        switch envelope.event {
        case .userMessage(let text):
            userMessage(text, images: claimImages(text), at: at)
            turn = (at, .zero, 0)
        case .textDelta(let text):
            var next = draft ?? Draft()
            next.text += text
            draft = next
        case .thinkingDelta(let text):
            var next = draft ?? Draft()
            next.thinking += text
            draft = next
        case .toolCallStreaming(let tool):
            var next = draft ?? Draft()
            next.preparingTool = tool
            draft = next
        case .assistantMessage(let message):
            draft = nil
            assistantBlocks(message.content, at: at)
        case let .toolStarted(callId, _, _, _):
            toolStarted(callId, at: at)
        case let .toolFinished(callId, _, output, isError, millis):
            finish(callId, output: output, isError: isError, millis: millis)
        case let .permissionRequested(requestId, callId, tool, _, input):
            permissionRequested(requestId, callId: callId, tool: tool, input: input)
        case let .permissionResolved(requestId, allowed, decision, by):
            permissionResolved(requestId, allowed: allowed, decision: decision, by: by)
        case let .usage(usage, _, context):
            if let current = turn { turn = (current.started, current.usage + usage, context) }
        case .compacted(let summary):
            append(.compacted(id: nextKey(), summary: summary))
        case .modelChanged(let model):
            append(.modelChanged(id: nextKey(), model: model))
        case let .notice(level, message):
            append(.notice(id: nextKey(), level: level, message: message))
        case let .turnFinished(status, _):
            turnFinished(status, at: at)
        default:
            break
        }
    }

    /// Shows requests that were open before the transcript loaded inline.
    public mutating func claim(_ pending: [PendingApproval]) {
        for request in pending where approvals[request.id] == nil {
            permissionRequested(request.id, callId: request.callId, tool: request.tool, input: request.input)
        }
    }

    /// Links `agent_spawn` calls of saved history to subagents the daemon still knows, by label.
    public mutating func link(_ children: [AgentState]) {
        let linked = Set(spawnKeys.compactMap { spawn($0)?.child })
        var candidates = children.filter { !linked.contains($0.id) }
        for key in spawnKeys {
            guard var call = spawn(key), call.child == nil,
                  let index = candidates.firstIndex(where: { $0.label == call.label }) else { continue }
            call.child = candidates.remove(at: index).id
            replace(.subagent(call))
        }
    }

    // MARK: History

    private static let recalledTag = "<recalled-memories>"

    private mutating func userBlocks(_ blocks: [Block]) {
        var text: [String] = []
        var images: [ImageBlock] = []
        var attachments: [Attachment] = []
        var inAttachment = false
        for block in blocks {
            switch block {
            case let .toolResult(id, content, isError):
                finish(id, output: Block.resultText(content), isError: isError, millis: nil, images: content.images)
            case .image(let image) where inAttachment:
                attachments[attachments.count - 1].images.append(image)
                continue
            case .image(let image):
                images.append(image)
            case .text(let value) where value.hasPrefix(Self.recalledTag):
                append(.recalled(id: nextKey(), text: Self.recalledText(value)))
            case .text(let value) where Self.attachmentPath(value) != nil:
                attachments.append(Self.attachment(value))
                inAttachment = true
                continue
            case .text(let value):
                text.append(value)
            default:
                break
            }
            inAttachment = false
        }
        if !text.isEmpty || !images.isEmpty || !attachments.isEmpty {
            userMessage(text.joined(separator: "\n"), images: images, at: nil, attachments: attachments)
        }
    }

    private mutating func assistantBlocks(_ blocks: [Block], at: Date?) {
        for block in blocks {
            switch block {
            case .text(let text) where !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
                append(.assistant(AssistantText(id: nextKey(), markdown: text, at: at)))
            case .thinking(let text) where !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
                append(.thinking(id: nextKey(), text: text))
            case let .toolUse(id, name, input):
                toolUse(id: id, name: name, input: input, at: at)
            default:
                break
            }
        }
    }

    private mutating func userMessage(_ text: String, images: [ImageBlock], at: Date?, attachments: [Attachment] = []) {
        draft = nil
        var typed = text
        var chips = attachments
        // A live `user_message` ends with "[attached: a.txt, src]" instead of the blocks.
        if let match = text.firstMatch(of: #/\n?\[attached: ([^\]]*)\]\s*$/#) {
            typed = String(text[..<match.range.lowerBound])
            if chips.isEmpty {
                chips = match.1.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.map { Attachment(path: $0) }
            }
        }
        append(.user(UserMessage(id: nextKey(), text: typed, images: images, attachments: chips, at: at)))
    }

    // MARK: Tool calls

    private mutating func toolUse(id: String, name: String, input: JSONValue, at: Date?) {
        let key = Self.toolKey(id)
        guard positions[key] == nil else { return }
        switch name {
        case "agent_spawn":
            let label = input["label"]?.stringValue ?? input["agent"]?.stringValue ?? "agent"
            let type = input["agent"]?.stringValue
            append(.subagent(SubagentCall(
                key: key, id: id, label: label, task: input["task"]?.stringValue ?? "",
                agentType: type == label ? nil : type, readOnly: input["read_only"]?.boolValue ?? false
            )))
            spawnKeys.append(key)
        case "todo":
            if let previous = lastPlanKey, case .plan(var plan)? = item(previous) {
                plan.superseded = true
                replace(.plan(plan))
            }
            append(.plan(Plan(id: key, items: Self.todos(input))))
            lastPlanKey = key
        default:
            append(.tool(ToolCall(key: key, id: id, name: name, input: input, at: at)))
        }
    }

    private mutating func toolStarted(_ callId: String, at: Date) {
        switch item(Self.toolKey(callId)) {
        case .tool(var call)?:
            if call.state == .pending { call.state = .running }
            call.at = at
            replace(.tool(call))
        case .subagent(var call)?:
            call.started = true
            replace(.subagent(call))
        default:
            break
        }
    }

    private mutating func finish(_ callId: String, output: String, isError: Bool, millis: Int?, images: [ImageBlock] = []) {
        switch item(Self.toolKey(callId)) {
        case .tool(var call)?:
            call.state = isError ? .failed : .done
            call.requestId = nil
            call.output = output
            call.millis = millis ?? call.millis
            if !images.isEmpty { call.images = images }
            if call.name == "send_to_user", !isError { call.shared = SharedFiles.parse(output) }
            replace(.tool(call))
        case .subagent(var call)?:
            call.done = true
            call.isError = isError
            call.output = output
            call.millis = millis
            call.child = call.child ?? ToolPresentation.spawnedAgentId(output)
            replace(.subagent(call))
        case .plan(var plan)? where isError:
            plan.error = Format.firstLine(output)
            replace(.plan(plan))
        default:
            break
        }
    }

    private mutating func permissionRequested(_ requestId: String, callId: String, tool: String, input: JSONValue) {
        let direct = Self.toolKey(callId)
        let key: String
        if case .tool? = item(direct) {
            key = direct
        } else if let match = matchCard(tool: tool, input: input) {
            key = match
        } else {
            return
        }
        guard case .tool(var call)? = item(key) else { return }
        approvals[requestId] = key
        call.state = .awaitingApproval
        call.requestId = requestId
        replace(.tool(call))
    }

    private mutating func permissionResolved(_ requestId: String, allowed: Bool, decision: Decision?, by: String?) {
        guard let key = approvals.removeValue(forKey: requestId), case .tool(var call)? = item(key) else { return }
        call.requestId = nil
        call.decision = decision
        call.decidedBy = by
        if allowed {
            call.state = .running
        } else {
            call.state = .denied
            call.output = "Denied"
        }
        replace(.tool(call))
    }

    // A saved answer, matched to its call by id. A denied call's result is an error; it reads as denied.
    private mutating func decided(_ record: DecisionRecord) {
        guard let decision = record.decision, case .tool(var call)? = item(Self.toolKey(record.callId)) else { return }
        call.decision = decision
        call.decidedBy = record.by
        if decision == .deny { call.state = .denied }
        replace(.tool(call))
    }

    // Requests that were open before this transcript loaded may carry no call id.
    private func matchCard(tool: String, input: JSONValue) -> String? {
        for item in items.reversed() {
            if case .tool(let call) = item, !call.isFinished, call.requestId == nil, call.name == tool, call.input == input {
                return call.key
            }
        }
        return nil
    }

    // MARK: Subagents

    private mutating func childStarted(_ child: String, label: String) {
        let open = spawnKeys.compactMap { key in spawn(key).flatMap { $0.child == nil ? $0 : nil } }
        let target = open.last { $0.started && $0.label == label } ?? open.last { $0.started } ?? open.last
        if var call = target {
            call.child = child
            replace(.subagent(call))
            return
        }
        let key = nextKey()
        append(.subagent(SubagentCall(key: key, id: key, label: label, task: "", agentType: nil, readOnly: false, child: child, started: true)))
        spawnKeys.append(key)
    }

    private func spawn(_ key: String) -> SubagentCall? {
        if case .subagent(let call)? = item(key) { return call }
        return nil
    }

    // MARK: Turns

    private mutating func turnFinished(_ status: TurnStatus, at: Date) {
        if let interrupted = draft {
            if !interrupted.thinking.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                append(.thinking(id: nextKey(), text: interrupted.thinking))
            }
            if !interrupted.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                append(.assistant(AssistantText(id: nextKey(), markdown: interrupted.text, at: at, interrupted: true)))
            }
        }
        draft = nil
        for index in items.indices {
            switch items[index] {
            case .tool(var call) where !call.isFinished:
                call.state = .skipped
                call.requestId = nil
                items[index] = .tool(call)
            case .subagent(var call) where !call.done && call.child == nil:
                call.abandoned = true
                items[index] = .subagent(call)
            default:
                break
            }
        }
        approvals = [:]
        if status != .completed || turn != nil {
            append(.turnEnd(TurnSummary(
                id: nextKey(), status: status, at: at,
                milliseconds: turn.map { Int(at.timeIntervalSince($0.started) * 1000) },
                usage: turn?.usage, contextTokens: turn?.context ?? 0
            )))
        }
        turn = nil
    }

    // MARK: Rows

    private func item(_ key: String) -> TranscriptItem? {
        positions[key].map { items[$0] }
    }

    private mutating func append(_ item: TranscriptItem) {
        positions[item.id] = items.count
        items.append(item)
    }

    private mutating func replace(_ item: TranscriptItem) {
        guard let index = positions[item.id] else { return }
        items[index] = item
    }

    private mutating func nextKey() -> String {
        defer { counter += 1 }
        return "row\(counter)"
    }

    private static func toolKey(_ callId: String) -> String { "call:\(callId)" }

    // The `user_message` event carries only text, so images sent from this app are matched back by it.
    private mutating func claimImages(_ text: String) -> [ImageBlock] {
        guard let index = outgoing.firstIndex(where: { text.hasPrefix($0.text) }) else { return [] }
        return outgoing.remove(at: index).images
    }

    private static func todos(_ input: JSONValue) -> [Todo] {
        (input["items"]?.arrayValue ?? []).compactMap { item in
            guard let content = item["content"]?.stringValue else { return nil }
            return Todo(content: content, status: item["status"]?.stringValue.flatMap(TodoStatus.init(rawValue:)) ?? .pending)
        }
    }

    private static func attachmentPath(_ text: String) -> String? {
        text.prefixMatch(of: #/<attachment path="([^"]*)"[^>]*>/#).map { String($0.1) }
    }

    private static func attachment(_ text: String) -> Attachment {
        guard let match = text.prefixMatch(of: #/<attachment path="([^"]*)"[^>]*>/#) else { return Attachment(path: text) }
        var body = String(text[match.range.upperBound...])
        if let end = body.firstMatch(of: #/<\/attachment>\s*$/#) { body = String(body[..<end.range.lowerBound]) }
        body = body.trimmingCharacters(in: .whitespacesAndNewlines)
        return Attachment(path: String(match.1), body: body.isEmpty ? nil : body)
    }

    private static func recalledText(_ text: String) -> String {
        var body = String(text.dropFirst(recalledTag.count))
        if let end = body.firstMatch(of: #/\s*<\/recalled-memories>\s*$/#) { body = String(body[..<end.range.lowerBound]) }
        return body.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

private extension [Block] {
    var images: [ImageBlock] {
        compactMap { block in
            guard case .image(let image) = block else { return nil }
            return image
        }
    }
}
