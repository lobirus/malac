import MalacKit
import SwiftUI

/// The tabs of one workspace. Opening it switches the connection to it.
struct WorkspaceScreen: View {
    let session: DaemonSession
    let workspaceId: String

    var body: some View {
        let state = session.state
        let view = state.workspace?.id == workspaceId ? state.workspace : nil
        List {
            if let view {
                ForEach(view.tabs) { tab in
                    NavigationLink(value: Route.tab(tab.id)) {
                        TabRow(tab: tab, phase: state.phase(of: tab), agents: state.agents(of: tab).count,
                               active: tab.id == view.activeTab, home: state.daemon?.userHome)
                    }
                }
                .listRowBackground(Palette.surface)
            }
        }
        .malacList()
        .overlay {
            if view == nil {
                ProgressView()
            } else if view?.tabs.isEmpty == true {
                ContentUnavailableView("No tabs", systemImage: "folder", description: Text("Open a project in this workspace on the daemon's machine."))
            }
        }
        .navigationTitle(view?.name ?? state.workspaces.first { $0.id == workspaceId }?.name ?? "Workspace")
        .onAppear { session.open(workspace: workspaceId) }
    }
}

private struct TabRow: View {
    let tab: WorkspaceTab
    let phase: Phase
    let agents: Int
    let active: Bool
    let home: String?

    var body: some View {
        HStack(spacing: 12) {
            StatusDot(phase: phase)
            VStack(alignment: .leading, spacing: 2) {
                Text(tab.name.isEmpty ? Format.baseName(tab.cwd) : tab.name).font(.body.weight(.medium))
                Text(Format.displayPath(tab.cwd, home: home))
                    .font(.footnote.monospaced())
                    .foregroundStyle(Palette.text2)
                    .lineLimit(1)
                    .truncationMode(.head)
                Text([Format.plural(agents, "agent"), tab.mode.title, phase == .idle ? nil : phase.label].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(Palette.text3)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}
