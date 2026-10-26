import MalacKit
import SwiftUI

/// An agent's transcript with the composer, or a composer for a new agent in a tab.
struct AgentScreen: View {
    let session: DaemonSession
    @State var agentId: String?
    let tabId: String?

    var body: some View {
        Group {
            if let agentId {
                TranscriptScreen(session: session, agentId: agentId)
                    .id(agentId)
            } else {
                NewAgentScreen(session: session, tabId: tabId) { agentId = $0 }
            }
        }
    }
}

private struct TranscriptScreen: View {
    let session: DaemonSession
    let agentId: String
    @State private var model: TranscriptModel
    @State private var pinned = true

    init(session: DaemonSession, agentId: String) {
        self.session = session
        self.agentId = agentId
        _model = State(initialValue: TranscriptModel(session: session, agent: agentId))
    }

    var body: some View {
        let state = session.state
        let agent = state.agents[agentId]
        let tab = state.tab(of: agentId)
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    if model.loading {
                        ProgressView("Loading the transcript").frame(maxWidth: .infinity).padding(.top, 40)
                    } else if let error = model.error {
                        Label("Could not load the saved transcript: \(error)", systemImage: "exclamationmark.triangle")
                            .font(.footnote).foregroundStyle(Palette.red)
                    }
                    ForEach(model.items) { item in
                        TranscriptRow(item: item, session: session, cwd: agent?.cwd)
                    }
                    if let draft = model.draft {
                        DraftView(draft: draft)
                    }
                    if !model.loading, model.items.isEmpty, model.draft == nil {
                        Text(agent?.live == true ? "Waiting for the first message." : "This session has no messages yet.")
                            .font(.callout).foregroundStyle(Palette.text3)
                            .frame(maxWidth: .infinity).padding(.top, 40)
                    }
                    Color.clear.frame(height: 1).id("end")
                        .onAppear { pinned = true }
                        .onDisappear { pinned = false }
                }
                .padding(16)
            }
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .background(Palette.page)
            .onChange(of: model.items.count) { follow(proxy) }
            .onChange(of: model.draft?.text.count) { follow(proxy) }
            .overlay(alignment: .bottomTrailing) {
                if !pinned {
                    Button {
                        withAnimation { proxy.scrollTo("end", anchor: .bottom) }
                    } label: {
                        Image(systemName: "arrow.down")
                            .padding(10)
                            .background(Palette.surface, in: Circle())
                            .overlay(Circle().strokeBorder(Palette.border))
                    }
                    .buttonStyle(.plain)
                    .padding(16)
                    .accessibilityLabel("Jump to the latest message")
                }
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            Composer(session: session, agent: agent, tab: tab, onSend: model.expect)
        }
        .navigationTitle(agent?.displayName ?? "Agent")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 0) {
                    Text(agent?.displayName ?? "Agent").font(.headline).lineLimit(1)
                    if let agent {
                        StatusLabel(phase: state.phase(of: agent)).font(.caption)
                    }
                }
                .accessibilityElement(children: .combine)
            }
        }
        .task(id: state.generation) { await model.load() }
        .onChange(of: session.revision) { model.rebuild() }
    }

    private func follow(_ proxy: ScrollViewProxy) {
        guard pinned else { return }
        proxy.scrollTo("end", anchor: .bottom)
    }
}

private struct NewAgentScreen: View {
    let session: DaemonSession
    let tabId: String?
    let started: (String) -> Void

    var body: some View {
        let tab = session.state.tab(tabId)
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "sparkles").font(.largeTitle).foregroundStyle(Palette.text3)
            Text("New agent").font(.title3.weight(.semibold))
            if let tab {
                Text("in \(tab.name) · \(Format.displayPath(tab.cwd, home: session.state.daemon?.userHome))")
                    .font(.footnote).foregroundStyle(Palette.text2)
                    .multilineTextAlignment(.center)
            }
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .padding()
        .background(Palette.page)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            Composer(session: session, agent: nil, tab: tab, onStarted: started)
        }
        .navigationTitle("New agent")
        .navigationBarTitleDisplayMode(.inline)
    }
}
