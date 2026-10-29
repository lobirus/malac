import Foundation
import MalacKit
import Observation

/// One agent's transcript: its saved history plus the events since, rebuilt
/// off the main thread whenever the session publishes.
@MainActor
@Observable
final class TranscriptModel {
    private(set) var items: [TranscriptItem] = []
    private(set) var draft: Draft?
    private(set) var loading = true
    private(set) var error: String?

    @ObservationIgnored private let session: DaemonSession
    @ObservationIgnored let agent: String
    @ObservationIgnored private var history: [Message] = []
    @ObservationIgnored private var decisions: [DecisionRecord] = []
    /// Requests that were waiting when the history was read.
    @ObservationIgnored private var openAtCutoff: [PendingApproval] = []
    @ObservationIgnored private var cutoff = -1
    @ObservationIgnored private var loadedGeneration = -1
    @ObservationIgnored private var sentImages: [(text: String, images: [ImageBlock])] = []
    @ObservationIgnored private var building: Task<Void, Never>?
    @ObservationIgnored private var built: Inputs?
    /// A snapshot cleared the logs and the history is being fetched again; keep showing the last build.
    @ObservationIgnored private var fetching = false

    /// What a transcript depends on besides its history; merged deltas change the last entry's seq.
    private struct Inputs: Equatable {
        var generation: Int
        var entries: Int
        var lastSeq: Int?
        var pending: [String]
        var children: [String]
        var sent: Int
    }

    init(session: DaemonSession, agent: String) {
        self.session = session
        self.agent = agent
    }

    /// Fetches the saved history; again after every snapshot, which clears the event logs.
    func load() async {
        let generation = session.state.generation
        guard generation != loadedGeneration else { return }
        loadedGeneration = generation
        var saved: [Message] = []
        var answers: [DecisionRecord] = []
        var open: [PendingApproval] = []
        var cutoff = -1
        var failure: String?
        if !session.state.sawCreation(of: agent) {
            cutoff = session.state.lastSeq
            open = session.state.pending(for: agent)
            fetching = true
            defer { fetching = false }
            do {
                let transcript = try await session.api.transcript(agent)
                saved = transcript.history
                answers = transcript.decisions
            } catch DaemonError.http(404, _) {
                // A brand-new agent has no saved transcript yet.
            } catch {
                failure = error.localizedDescription
            }
        }
        history = saved
        decisions = answers
        openAtCutoff = open
        self.cutoff = cutoff
        error = failure
        built = nil
        loading = false
        rebuild()
    }

    /// Images this app sent: `user_message` carries only text, so they are matched back by it.
    func expect(_ images: [ImageBlock], with text: String) {
        guard !images.isEmpty else { return }
        sentImages.append((text, images))
    }

    func rebuild() {
        guard !loading, !fetching else { return }
        let state = session.state
        let history = history
        let decisions = decisions
        let openAtCutoff = openAtCutoff
        let cutoff = cutoff
        let log = state.log(agent)
        let pending = state.pending(for: agent)
        let children = state.children(of: agent)
        let sent = sentImages
        let inputs = Inputs(generation: loadedGeneration, entries: log.count, lastSeq: log.last?.seq,
                            pending: pending.map(\.id), children: children.map(\.id), sent: sent.count)
        guard inputs != built else { return }
        built = inputs
        building?.cancel()
        building = Task {
            let transcript = await Task.detached(priority: .userInitiated) {
                Transcript(history: history, decisions: decisions, log: log, after: cutoff, openAtCutoff: openAtCutoff,
                           pending: pending, children: children, sentImages: sent)
            }.value
            guard !Task.isCancelled else { return }
            items = transcript.items
            draft = transcript.draft
        }
    }
}
