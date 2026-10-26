import MalacKit
import SwiftUI

/// Where a push in the workspaces stack leads.
enum Route: Hashable {
    case workspace(String)
    case tab(String)
    case agent(String)
    case newAgent(tab: String)
    case job(String)
}

/// Workspaces → tabs → agents → transcript.
struct WorkspacesTab: View {
    @Environment(AppModel.self) private var app
    @State private var path: [Route] = []

    var body: some View {
        NavigationStack(path: $path) {
            SessionContent { session in
                WorkspaceList(session: session)
                    .navigationDestination(for: Route.self) { route in
                        destination(route, session: session)
                    }
            }
            .navigationTitle(app.session?.daemon.name ?? "Workspaces")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { DaemonMenu() }
            }
        }
        .onChange(of: app.destination) { _, destination in
            guard let destination else { return }
            path = [.agent(destination.agent)]
            app.destination = nil
        }
        .onChange(of: app.session?.daemon.id) { path = [] }
    }

    @ViewBuilder
    private func destination(_ route: Route, session: DaemonSession) -> some View {
        switch route {
        case .workspace(let id): WorkspaceScreen(session: session, workspaceId: id)
        case .tab(let id): TabScreen(session: session, tabId: id)
        case .agent(let id): AgentScreen(session: session, agentId: id, tabId: nil)
        case .newAgent(let tab): AgentScreen(session: session, agentId: nil, tabId: tab)
        case .job(let id): JobLogView(session: session, jobId: id)
        }
    }
}

/// Switches between paired daemons from the navigation bar.
struct DaemonMenu: View {
    @Environment(AppModel.self) private var app

    var body: some View {
        if app.daemons.count > 1 {
            Menu {
                Picker("Daemon", selection: Binding(get: { app.selected?.id }, set: { id in
                    app.select(app.daemons.first { $0.id == id })
                })) {
                    ForEach(app.daemons) { Text($0.name).tag(Optional($0.id)) }
                }
            } label: {
                Label("Switch daemon", systemImage: "server.rack")
            }
        }
    }
}

private struct WorkspaceList: View {
    let session: DaemonSession

    var body: some View {
        let state = session.state
        List {
            ForEach(state.workspaces.sorted { ($0.updated ?? .distantPast) > ($1.updated ?? .distantPast) }) { workspace in
                NavigationLink(value: Route.workspace(workspace.id)) {
                    WorkspaceRow(workspace: workspace, phase: state.phase(of: workspace), current: workspace.id == state.workspace?.id)
                }
            }
            .listRowBackground(Palette.surface)
        }
        .malacList()
        .overlay {
            if state.isLoaded, state.workspaces.isEmpty {
                ContentUnavailableView("No workspaces yet", systemImage: "square.stack.3d.up",
                                       description: Text("Start malac in a project on the daemon's machine, or run `malac --cwd <folder>`."))
            } else if !state.isLoaded {
                ProgressView()
            }
        }
        .refreshable { session.reconnectNow() }
    }
}

private struct WorkspaceRow: View {
    let workspace: WorkspaceSummary
    let phase: Phase
    let current: Bool

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(phase: phase)
            VStack(alignment: .leading, spacing: 2) {
                Text(workspace.name.isEmpty ? "Workspace" : workspace.name).font(.body.weight(.medium))
                Text(detail).font(.footnote).foregroundStyle(Palette.text2)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    private var detail: String {
        var parts = [Format.plural(workspace.tabs, "tab")]
        if workspace.running > 0 { parts.append("\(workspace.running) running") }
        if phase == .awaitingApproval { parts.append("needs approval") }
        if let updated = workspace.updated { parts.append(updated.formatted(.relative(presentation: .named))) }
        return parts.joined(separator: " · ")
    }
}
