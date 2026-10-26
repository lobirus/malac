import MalacKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @State private var pairing = false

    var body: some View {
        @Bindable var app = app
        Group {
            if app.daemons.isEmpty {
                WelcomeView { pairing = true }
            } else {
                TabView(selection: $app.tab) {
                    WorkspacesTab()
                        .tabItem { Label("Workspaces", systemImage: "square.stack.3d.up") }
                        .tag(AppModel.Tab.workspaces)
                    ApprovalsTab()
                        .tabItem { Label("Approvals", systemImage: "checkmark.shield") }
                        .badge(app.session?.state.pending.count ?? 0)
                        .tag(AppModel.Tab.approvals)
                    JobsTab()
                        .tabItem { Label("Jobs", systemImage: "terminal") }
                        .tag(AppModel.Tab.jobs)
                    DaemonsTab()
                        .tabItem { Label("Daemons", systemImage: "server.rack") }
                        .tag(AppModel.Tab.daemons)
                }
            }
        }
        .background(Palette.page)
        .sheet(isPresented: $pairing) { PairView() }
        .sheet(item: $app.repairing) { daemon in PairView(again: daemon) }
    }
}

/// The first screen, before any daemon is paired.
private struct WelcomeView: View {
    let pair: () -> Void

    var body: some View {
        VStack(spacing: 20) {
            Spacer()
            PigMark()
                .frame(width: 96)
                .foregroundStyle(Palette.text)
            Text("malac").font(.largeTitle.weight(.semibold))
            Text("Watch and steer the agents on your computer or server. On the machine running the daemon, run `malac remote pair` and scan the code it shows.")
                .font(.callout)
                .foregroundStyle(Palette.text2)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)
            Button(action: pair) {
                Text("Pair with a daemon").frame(maxWidth: 320)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .background(Palette.page)
    }
}

/// A tab's content for the selected daemon, or a note when there is none.
struct SessionContent<Content: View>: View {
    @Environment(AppModel.self) private var app
    @ViewBuilder let content: (DaemonSession) -> Content

    var body: some View {
        if let session = app.session {
            content(session)
                .safeAreaInset(edge: .top, spacing: 0) { ConnectionBanner(session: session) }
                .overlay(alignment: .bottom) { NoticeToast(session: session) }
        } else {
            ContentUnavailableView("No daemon selected", systemImage: "server.rack",
                                   description: Text("Choose one in Daemons."))
        }
    }
}

/// A daemon notice or a failed command, shown briefly.
private struct NoticeToast: View {
    @Bindable var session: DaemonSession

    var body: some View {
        if let notice = session.notice {
            Label(notice.text, systemImage: notice.level == .info ? "info.circle" : "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(notice.level == .error ? Palette.red : notice.level == .warning ? Palette.amber : Palette.text)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .card(Palette.surface)
                .padding(.horizontal, 16)
                .padding(.bottom, 12)
                .onTapGesture { session.notice = nil }
                .task(id: notice.id) {
                    try? await Task.sleep(for: .seconds(4))
                    if session.notice?.id == notice.id { session.notice = nil }
                }
                .accessibilityAddTraits(.isStaticText)
                .transition(.move(edge: .bottom).combined(with: .opacity))
        }
    }
}
