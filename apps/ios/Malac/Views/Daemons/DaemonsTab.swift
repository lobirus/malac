import MalacKit
import SwiftUI
import UserNotifications

/// The paired daemons, the selected one's remote access and keep-awake, and notifications.
struct DaemonsTab: View {
    @Environment(AppModel.self) private var app
    @State private var pairing = false
    @State private var forgetting: SavedDaemon?
    @State private var renaming: SavedDaemon?
    @State private var newName = ""
    @State private var notifications = UNAuthorizationStatus.notDetermined

    var body: some View {
        NavigationStack {
            List {
                Section("Paired daemons") {
                    ForEach(app.daemons) { daemon in
                        DaemonRow(daemon: daemon, session: app.session?.daemon.id == daemon.id ? app.session : nil)
                            .contentShape(Rectangle())
                            .onTapGesture { app.select(daemon) }
                            .accessibilityAddTraits(.isButton)
                            .swipeActions {
                                Button("Forget", role: .destructive) { forgetting = daemon }
                                Button("Rename") {
                                    newName = daemon.name
                                    renaming = daemon
                                }
                            }
                            .contextMenu {
                                Button("Rename", systemImage: "pencil") {
                                    newName = daemon.name
                                    renaming = daemon
                                }
                                Button("Forget", systemImage: "trash", role: .destructive) { forgetting = daemon }
                            }
                    }
                    Button {
                        pairing = true
                    } label: {
                        Label("Pair a daemon", systemImage: "plus")
                    }
                }
                .listRowBackground(Palette.surface)

                if let session = app.session, session.state.isLoaded {
                    DaemonDetails(session: session)
                }

                Section {
                    LabeledContent("Notifications", value: notificationText)
                    if notifications == .notDetermined {
                        Button("Allow notifications") {
                            Task {
                                await app.notifier.requestAuthorization()
                                notifications = await app.notifier.authorizationStatus()
                            }
                        }
                    } else if notifications == .denied {
                        Button("Open Settings") {
                            if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                        }
                    }
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("While the app is open or was just left, approvals and finished turns arrive at once. Later, iOS lets the app check every so often, which can take a while.")
                }
                .listRowBackground(Palette.surface)
            }
            .malacList()
            .navigationTitle("Daemons")
            .task { notifications = await app.notifier.authorizationStatus() }
            .sheet(isPresented: $pairing) { PairView() }
            .confirmationDialog("Forget \(forgetting?.name ?? "")?", isPresented: .init(get: { forgetting != nil }, set: { if !$0 { forgetting = nil } }), titleVisibility: .visible) {
                Button("Forget", role: .destructive) {
                    if let daemon = forgetting { Task { await app.forget(daemon) } }
                }
            } message: {
                Text("This phone is unpaired from the daemon and its token is deleted.")
            }
            .alert("Rename", isPresented: .init(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Name", text: $newName)
                Button("Save") { if let daemon = renaming { app.rename(daemon, to: newName) } }
                Button("Cancel", role: .cancel) {}
            }
        }
    }

    private var notificationText: String {
        switch notifications {
        case .authorized, .provisional, .ephemeral: "On"
        case .denied: "Off"
        default: "Not asked yet"
        }
    }
}

private struct DaemonRow: View {
    let daemon: SavedDaemon
    let session: DaemonSession?

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(daemon.name).font(.body.weight(.medium))
                Text(daemon.host).font(.footnote).foregroundStyle(Palette.text2)
            }
            Spacer()
            if let session {
                Text(status(session)).font(.footnote).foregroundStyle(Palette.text2)
                Image(systemName: "checkmark").foregroundStyle(Palette.green).accessibilityLabel("Selected")
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func status(_ session: DaemonSession) -> String {
        switch session.status {
        case .online: session.state.daemon.map { "malac \($0.version)" } ?? "Online"
        case .connecting: "Connecting"
        case .waiting: "Unreachable"
        case .unauthorized: "Not paired"
        case .stopped: ""
        }
    }
}

/// Remote access, keep-awake and voice of the selected daemon.
private struct DaemonDetails: View {
    let session: DaemonSession

    /// "Dictation, talk mode", "Off", or that no model can do any of it.
    static func summary(_ voice: VoiceStatus) -> String {
        let ready = [("Dictation", voice.dictation), ("speech", voice.speech), ("talk mode", voice.talk)]
            .filter { $0.1.isReady }
            .map(\.0)
        if !ready.isEmpty {
            let text = ready.joined(separator: ", ")
            return text.prefix(1).uppercased() + text.dropFirst()
        }
        let available = [voice.dictation, voice.speech, voice.talk].contains { !$0.models.isEmpty }
        return available ? "Off" : "No voice models"
    }

    var body: some View {
        let state = session.state
        if let remote = state.remote {
            Section {
                LabeledContent("Remote access", value: remote.enabled ? "On" : "Off")
                if let tunnel = remote.tunnel {
                    LabeledContent("Tunnel", value: "\(tunnel.provider), \(tunnel.state)")
                }
                if let best = remote.urls.first {
                    LabeledContent("Address") {
                        Text(best.url).font(.footnote.monospaced()).textSelection(.enabled)
                    }
                }
                LabeledContent("Paired devices", value: String(remote.devices))
            } header: {
                Text(session.daemon.name)
            } footer: {
                Text(remote.enabled
                    ? "Revoke devices with `malac remote devices` and `malac remote revoke <id>` on the daemon's machine."
                    : "Only this machine, SSH tunnels and the devices on it can connect. Turn remote access on with `malac daemon start --remote`.")
            }
            .listRowBackground(Palette.surface)
        }
        if let awake = state.awake {
            Section {
                Picker("Keep awake", selection: Binding(get: { awake.mode }, set: { session.keepAwake($0) })) {
                    ForEach(KeepAwake.allCases) { Text($0.title).tag($0) }
                }
                LabeledContent("Now", value: awake.awake ? "Awake" : "May sleep")
                if let error = awake.error {
                    Text(error).font(.footnote).foregroundStyle(Palette.red)
                }
            } header: {
                Text("Keep awake")
            } footer: {
                Text(awake.reason.isEmpty ? "Until the daemon restarts; `daemon.keep_awake` sets the default." : "\(awake.reason.prefix(1).uppercased() + awake.reason.dropFirst()). The choice lasts until the daemon restarts.")
            }
            .listRowBackground(Palette.surface)
        }
        if let voice = state.voice {
            Section {
                NavigationLink {
                    VoiceSettingsView(session: session)
                } label: {
                    LabeledContent("Voice", value: Self.summary(voice))
                }
            } header: {
                Text("Voice")
            } footer: {
                Text("Dictation, speech and talk mode, for every app connected to this daemon.")
            }
            .listRowBackground(Palette.surface)
        }
        if let daemon = state.daemon {
            Section("About") {
                LabeledContent("Version", value: daemon.version)
                LabeledContent("Clients", value: String(daemon.clients))
                if daemon.versionMismatch || state.update?.installed != nil {
                    Text("A newer malac is installed. Run `malac daemon restart` to use it.")
                        .font(.footnote).foregroundStyle(Palette.amber)
                }
            }
            .listRowBackground(Palette.surface)
        }
    }
}
