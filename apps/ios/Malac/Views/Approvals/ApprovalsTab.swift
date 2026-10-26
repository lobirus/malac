import MalacKit
import SwiftUI

/// Every pending approval of the selected daemon, across workspaces. A request
/// leaves the list as soon as any client answers it.
struct ApprovalsTab: View {
    @State private var path: [Route] = []

    var body: some View {
        NavigationStack(path: $path) {
            SessionContent { session in
                ApprovalList(session: session) { request in
                    if let workspace = session.state.workspace(of: request) { session.open(workspace: workspace) }
                    path = [.agent(request.agent)]
                }
                .navigationDestination(for: Route.self) { route in
                    switch route {
                    case .agent(let id): AgentScreen(session: session, agentId: id, tabId: nil)
                    default: EmptyView()
                    }
                }
            }
            .navigationTitle("Approvals")
        }
    }
}

private struct ApprovalList: View {
    let session: DaemonSession
    let open: (PendingApproval) -> Void

    var body: some View {
        let state = session.state
        List {
            ForEach(state.pending) { request in
                ApprovalRow(session: session, request: request, place: state.place(of: request), open: { open(request) })
            }
            .listRowBackground(Palette.surface)
        }
        .malacList()
        .animation(.default, value: state.pending.map(\.id))
        .overlay {
            if state.isLoaded, state.pending.isEmpty {
                ContentUnavailableView("Nothing to approve", systemImage: "checkmark.shield",
                                       description: Text("Requests from every workspace show up here."))
            }
        }
    }
}

private struct ApprovalRow: View {
    let session: DaemonSession
    let request: PendingApproval
    let place: String
    let open: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button(action: open) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Image(systemName: ToolRow.symbol(request.tool)).foregroundStyle(Palette.amber).accessibilityHidden(true)
                        Text(ToolPresentation.title(request.tool).title).font(.body.weight(.semibold))
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption).foregroundStyle(Palette.text3).accessibilityHidden(true)
                    }
                    Text(request.summary.isEmpty ? ToolPresentation.summary(request.tool, input: request.input) : request.summary)
                        .font(.callout.monospaced())
                        .lineLimit(4)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text([request.agentLabel, place].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.footnote)
                        .foregroundStyle(Palette.text2)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint("Opens the agent's transcript")
            ApprovalBar(session: session, requestId: request.id)
        }
        .padding(.vertical, 4)
    }
}
