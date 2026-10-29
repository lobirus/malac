import Foundation
import MalacKit
import Observation

/// The selected daemon: its connection, the state it keeps current, and the
/// commands the views send.
@MainActor
@Observable
final class DaemonSession {
    struct Notice: Identifiable, Equatable {
        let id = UUID()
        var level: Level
        var text: String
    }

    let daemon: SavedDaemon
    let api: DaemonAPI
    /// Plays what the daemon reads aloud on this device.
    let speech: SpeechPlayer

    /// Published at most every 50 ms, so a stream of deltas does not redraw every list.
    private(set) var state = DaemonState()
    /// Bumped with every publish; transcripts rebuild when it changes.
    private(set) var revision = 0
    private(set) var status = DaemonConnection.Status.stopped
    var notice: Notice?
    /// Requests this app answered and is waiting to hear back about.
    private(set) var answering: Set<String> = []

    /// Set by the app: notifications are posted only while it is in the background,
    /// and speech plays only while it is on screen.
    @ObservationIgnored var inForeground = true {
        didSet {
            if !inForeground { speech.stop() }
            syncListening()
        }
    }
    @ObservationIgnored private let connection: DaemonConnection
    @ObservationIgnored private let notifier: Notifier
    @ObservationIgnored private var working = DaemonState()
    @ObservationIgnored private var publishing: Task<Void, Never>?
    /// The workspace the user opened; a reconnect's snapshot is of the daemon's last used one.
    @ObservationIgnored private var browsing: String?
    /// What this connection last told the daemon with `voice_listen`; nil on a new connection.
    @ObservationIgnored private var listening: Bool?

    /// "Speak on this device", stored per device and on unless switched off.
    static let speakHereKey = "voice.speakHere"
    static var speakHere: Bool { UserDefaults.standard.object(forKey: speakHereKey) as? Bool ?? true }

    init(daemon: SavedDaemon, endpoint: DaemonEndpoint, notifier: Notifier) {
        self.daemon = daemon
        self.notifier = notifier
        api = DaemonAPI(endpoint: endpoint)
        speech = SpeechPlayer(api: api)
        connection = DaemonConnection(endpoint: endpoint)
        connection.onMessage = { [weak self] in self?.receive($0) }
        connection.onStatus = { [weak self] in self?.changed($0) }
    }

    var isOnline: Bool { status == .online }

    func start() { connection.start() }

    func stop() {
        connection.stop()
        speech.stop()
    }

    func reconnectNow() { connection.reconnectNow() }

    // MARK: Messages

    private func changed(_ status: DaemonConnection.Status) {
        self.status = status
        guard status != .online else { return }
        // The next connection is a new listener, or none until it asks.
        listening = nil
        working.forgetListener()
    }

    private func receive(_ message: ServerMessage) {
        for effect in working.apply(message) { handle(effect) }
        switch message {
        case .snapshot, .voice: syncListening()
        default: break
        }
        if case .snapshot(let snapshot) = message {
            if let browsing, snapshot.clientRef == nil, snapshot.workspace?.id != browsing {
                if snapshot.workspaces.contains(where: { $0.id == browsing }) {
                    connection.send(.snapshot(workspace: browsing))
                } else {
                    self.browsing = nil
                }
            }
            publish()
        } else if publishing == nil {
            publishing = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(50))
                self?.publish()
            }
        }
    }

    private func publish() {
        publishing?.cancel()
        publishing = nil
        state = working
        revision += 1
    }

    private func handle(_ effect: Effect) {
        switch effect {
        case .resync(let workspace):
            connection.send(.snapshot(workspace: workspace))
        case let .notice(level, text):
            notice = Notice(level: level, text: text)
        case .approvalRequested(let request):
            if !inForeground { notifier.approval(request, place: working.place(of: request), daemon: daemon) }
        case .approvalResolved(let id):
            answering.remove(id)
            notifier.removeApproval(id)
        case let .turnFinished(agent, status, text):
            if !inForeground { notifier.turnFinished(agent: agent.id, name: agent.displayName, status: status, text: text, daemon: daemon) }
        case .speak(let line):
            if inForeground, Self.speakHere { speech.enqueue(line) }
        case .stopSpeaking(let ids):
            speech.stop { ids.contains($0.id) }
        }
    }

    // MARK: Voice

    /// Tells the daemon whether this device plays speech: when "Speak on this
    /// device" is on, speech is ready and the app is on screen. Again after
    /// every reconnect, since listeners are per connection.
    func syncListening() {
        guard isOnline else { return }
        let wanted = Self.speakHere && inForeground && working.voice?.speech.isReady == true
        guard wanted != (listening ?? false) else { return }
        listening = wanted
        if !wanted { speech.stop() }
        Task { _ = try? await connection.request(.voiceListen(on: wanted)) }
    }

    /// Changes a voice setting on the daemon, for every client.
    func setVoice(_ change: VoiceSetting) async {
        do {
            working.setVoice(try await api.setVoice([change]))
            publish()
            syncListening()
        } catch {
            notice = Notice(level: .error, text: "Could not change the voice settings: \(error.localizedDescription)")
        }
    }

    /// Reads the providers' model lists again.
    func refreshVoice() async {
        do {
            working.setVoice(try await api.voice(refresh: true))
            publish()
            syncListening()
        } catch {
            notice = Notice(level: .error, text: "Could not check the voice models: \(error.localizedDescription)")
        }
    }

    // MARK: Commands

    func approve(_ requestId: String, _ decision: Decision) async {
        answering.insert(requestId)
        // Its announcement is stale now; this stops it before the daemon says so.
        speech.stop { $0.reason == "approval" }
        do {
            _ = try await connection.request(.approve(requestId: requestId, decision: decision))
        } catch DaemonError.command(let message) {
            // Another client answered first; the request is settled either way.
            answering.remove(requestId)
            working.removeRequest(requestId)
            publish()
            notice = Notice(level: .info, text: message)
        } catch {
            answering.remove(requestId)
            notice = Notice(level: .error, text: error.localizedDescription)
        }
    }

    /// Prompts `agent`, or starts a new agent in `tab`. Returns the agent that got the prompt.
    func prompt(_ text: String, images: [ImageInput], agent: String?, tab: String?, model: String?) async throws -> String? {
        let reply = try await connection.request(.prompt(text: text, agent: agent, tab: agent == nil ? tab : nil, images: images, model: agent == nil ? model : nil))
        // The new agent's first events came before the reply; show them before its screen opens.
        publish()
        guard case .ack(let ack) = reply else { return agent }
        return ack.agent ?? agent
    }

    func cancel(_ agent: String) {
        connection.send(.cancel(agent: agent))
    }

    func setMode(_ mode: Mode, tab: String) {
        connection.send(.mode(mode, tab: tab))
    }

    func setModel(_ model: String, agent: String) {
        connection.send(.model(agent: agent, model: model))
    }

    func keepAwake(_ mode: KeepAwake) {
        connection.send(.keepAwake(mode))
    }

    /// Shows another workspace; unlike `workspace_open` this leaves the daemon's last used one alone.
    func open(workspace id: String) {
        browsing = id
        guard state.workspace?.id != id else { return }
        connection.send(.snapshot(workspace: id))
    }

    func jobOutput(_ job: String, tail: Int = 400) async throws -> JobOutput {
        guard case .jobOutput(_, let output) = try await connection.request(.jobOutput(job: job, tail: tail)) else {
            throw DaemonError.invalidResponse
        }
        return output
    }

    func kill(job: String) async {
        do {
            _ = try await connection.request(.jobKill(job: job))
        } catch {
            notice = Notice(level: .error, text: error.localizedDescription)
        }
    }

    /// What the user saw, so the background check does not notify it again.
    var ledger: Ledger { Ledger(seen: working) }
}
