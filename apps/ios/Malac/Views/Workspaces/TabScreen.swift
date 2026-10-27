import MalacKit
import SwiftUI

/// The agents of a tab, with their live subagents, and a button for a new agent.
struct TabScreen: View {
    let session: DaemonSession
    let tabId: String
    @State private var talking = false

    var body: some View {
        let state = session.state
        let tab = state.tab(tabId)
        List {
            if let tab {
                ForEach(state.agents(of: tab)) { agent in
                    AgentTree(session: session, agent: agent, depth: 0)
                }
                .listRowBackground(Palette.surface)
            }
        }
        .malacList()
        .overlay {
            if tab == nil {
                ContentUnavailableView("This tab was closed", systemImage: "folder.badge.minus")
            } else if tab?.agents.isEmpty == true {
                ContentUnavailableView("No agents yet", systemImage: "sparkles", description: Text("Start one with the button above."))
            }
        }
        .navigationTitle(tab?.name ?? "Tab")
        .toolbar {
            if let tab {
                ToolbarItem(placement: .topBarTrailing) {
                    ModeMenu(mode: tab.mode) { session.setMode($0, tab: tab.id) }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    NavigationLink(value: Route.newAgent(tab: tab.id)) {
                        Label("New agent", systemImage: "square.and.pencil")
                    }
                }
                if state.voice?.talk.isReady == true {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { talking = true } label: {
                            Label("Talk", systemImage: "headphones")
                        }
                        .disabled(!session.isOnline)
                        .accessibilityHint("Starts a spoken conversation through the tab's active agent")
                    }
                }
            }
        }
        .fullScreenCover(isPresented: $talking) {
            TalkView(session: session, tab: tabId, agent: nil, title: tab?.name ?? "Tab")
        }
    }
}

/// An agent row and, below it, its subagents.
private struct AgentTree: View {
    let session: DaemonSession
    let agent: AgentState
    let depth: Int

    var body: some View {
        NavigationLink(value: Route.agent(agent.id)) {
            AgentRow(agent: agent, phase: session.state.phase(of: agent))
                .padding(.leading, CGFloat(depth) * 16)
        }
        if depth < 4 {
            ForEach(session.state.children(of: agent.id)) { child in
                AgentTree(session: session, agent: child, depth: depth + 1)
            }
        }
    }
}

struct AgentRow: View {
    let agent: AgentState
    let phase: Phase

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(phase: phase)
            VStack(alignment: .leading, spacing: 2) {
                Text(agent.displayName).font(.body.weight(.medium)).lineLimit(2)
                Text(detail)
                    .font(.footnote)
                    .foregroundStyle(phase == .awaitingApproval ? Palette.amber : Palette.text2)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            if let progress {
                Text(progress).font(.caption.monospacedDigit()).foregroundStyle(Palette.text3)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityValue(phase.label)
    }

    private var detail: String {
        if phase == .running, !agent.activity.isEmpty { return agent.activity }
        if phase == .awaitingApproval { return agent.activity.isEmpty ? "Waiting for approval" : agent.activity.prefix(1).uppercased() + agent.activity.dropFirst() }
        var parts = [phase == .idle && !agent.live ? "Saved" : phase.label]
        if !agent.model.isEmpty { parts.append(agent.model) }
        let tokens = agent.usage.inputTokens + agent.usage.outputTokens
        if tokens > 0 { parts.append("\(Format.tokens(tokens)) tokens") }
        return parts.joined(separator: " · ")
    }

    private var progress: String? {
        guard phase == .running, !agent.todos.isEmpty else { return nil }
        return "\(agent.todos.filter { $0.status == .completed }.count)/\(agent.todos.count)"
    }
}

/// The tab's permission mode: ask, edits or autopilot.
struct ModeMenu: View {
    let mode: Mode
    let change: (Mode) -> Void

    var body: some View {
        Menu {
            Picker("Mode", selection: Binding(get: { mode }, set: change)) {
                ForEach(Mode.allCases) { mode in
                    Text(mode.title).tag(mode)
                }
            }
        } label: {
            Label("Mode: \(mode.title)", systemImage: mode == .autopilot ? "bolt.fill" : mode == .edits ? "pencil" : "hand.raised")
        }
        .accessibilityLabel("Permission mode, \(mode.title)")
    }
}
