import MalacKit
import SwiftUI

/// An `agent_spawn` call: the subagent's status, what it is doing, its
/// report, and a link to its own transcript.
struct SubagentCard: View {
    let call: SubagentCall
    let session: DaemonSession

    var body: some View {
        let child = call.child.flatMap { session.state.agents[$0] }
        let status = call.status(child: child)
        let phase = Phase(status)
        let report = call.report(child: child)
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "person.2").foregroundStyle(Palette.text2).accessibilityHidden(true)
                Text(call.label).font(.footnote.weight(.semibold))
                if let type = call.agentType { Tag(text: type) }
                if call.readOnly { Tag(text: "read-only") }
                Spacer(minLength: 4)
                StatusLabel(phase: call.abandoned ? .idle : phase)
                    .font(.caption)
            }
            if let meta = meta(child: child, status: status) {
                Text(meta).font(.caption.monospacedDigit()).foregroundStyle(Palette.text3)
            }
            if !call.task.isEmpty {
                Disclosure(title: Format.firstLine(call.task), icon: "text.alignleft") {
                    Text(call.task).font(.footnote).foregroundStyle(Palette.text2).textSelection(.enabled)
                }
            }
            if status == .running, let activity = child?.activity, !activity.isEmpty {
                Text(activity).font(.caption).foregroundStyle(Palette.text2).lineLimit(1)
            }
            if !report.isEmpty {
                MarkdownText(report).font(.footnote)
            }
            if let id = call.child {
                NavigationLink(value: Route.agent(id)) {
                    Label("Open transcript", systemImage: "chevron.right")
                        .labelStyle(TrailingIcon())
                        .font(.footnote.weight(.medium))
                }
                .tint(Palette.green)
            } else if call.abandoned {
                Text("Not started").font(.caption).foregroundStyle(Palette.text3)
            }
        }
        .padding(10)
        .card(Palette.surface)
        .accessibilityElement(children: .contain)
    }

    private func meta(child: AgentState?, status: AgentStatus) -> String? {
        var parts: [String] = []
        if let model = child?.model, !model.isEmpty { parts.append(model) }
        if let millis = call.millis, status != .running {
            parts.append(Format.duration(milliseconds: millis))
        }
        if let usage = child?.usage {
            let tokens = usage.inputTokens + usage.outputTokens
            if tokens > 0 { parts.append("\(Format.tokens(tokens)) tok") }
            let cost = Format.cost(usage.cost)
            if !cost.isEmpty { parts.append(cost) }
        }
        if let child {
            let nested = session.state.children(of: child.id).count
            if nested > 0 { parts.append(Format.plural(nested, "subagent")) }
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

private struct Tag: View {
    let text: String

    var body: some View {
        Text(text).font(.caption2).padding(.horizontal, 5).padding(.vertical, 1).card(Palette.surface3)
    }
}

private struct TrailingIcon: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.title
            configuration.icon.imageScale(.small)
        }
    }
}
