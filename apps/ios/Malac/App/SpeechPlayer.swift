import AVFoundation
import MalacKit
import Observation

/// Plays the sentences the daemon reads aloud on this device, one after
/// another: each clip is fetched as soon as it is announced and played when
/// the one before it ends.
@MainActor
@Observable
final class SpeechPlayer {
    /// The sentence playing now, shown while it plays.
    private(set) var current: SpokenLine?
    /// Set while dictation or talk mode has the audio session; sentences that arrive meanwhile are dropped.
    var paused = false {
        didSet { if paused { stop() } }
    }

    @ObservationIgnored private let api: DaemonAPI
    @ObservationIgnored private var queue: [(line: SpokenLine, audio: Task<Data?, Never>)] = []
    @ObservationIgnored private var player: AVAudioPlayer?
    @ObservationIgnored private let delegate = PlayerDelegate()
    @ObservationIgnored private var advancing: Task<Void, Never>?
    @ObservationIgnored private var activated = false

    init(api: DaemonAPI) {
        self.api = api
        delegate.finished = { [weak self] player in self?.finished(player) }
    }

    func enqueue(_ line: SpokenLine) {
        guard !paused else { return }
        let api = api
        queue.append((line, Task { try? await api.spoken(line.id).data }))
        if current == nil { next() }
    }

    /// Stops the sentence playing and drops the ones waiting.
    func stop() {
        advancing?.cancel()
        advancing = nil
        for entry in queue { entry.audio.cancel() }
        queue = []
        player?.stop()
        player = nil
        current = nil
        deactivate()
    }

    private func next() {
        guard !queue.isEmpty else {
            current = nil
            deactivate()
            return
        }
        let entry = queue.removeFirst()
        current = entry.line
        advancing = Task { [weak self] in
            let audio = await entry.audio.value
            guard let self, !Task.isCancelled else { return }
            if let audio, self.play(audio) { return }
            // The clip could not be fetched or played: show the sentence a moment instead.
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            self.next()
        }
    }

    private func play(_ audio: Data) -> Bool {
        let session = AVAudioSession.sharedInstance()
        do {
            // Spoken prompts lower other audio and pause spoken audio such as podcasts.
            try session.setCategory(.playback, mode: .voicePrompt, options: [.duckOthers, .interruptSpokenAudioAndMixWithOthers])
            try session.setActive(true)
            activated = true
            let player = try AVAudioPlayer(data: audio)
            player.delegate = delegate
            self.player = player
            return player.play()
        } catch {
            return false
        }
    }

    private func finished(_ id: ObjectIdentifier) {
        guard let player, ObjectIdentifier(player) == id else { return }
        self.player = nil
        next()
    }

    private func deactivate() {
        guard activated else { return }
        activated = false
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

private final class PlayerDelegate: NSObject, AVAudioPlayerDelegate {
    var finished: (@MainActor (ObjectIdentifier) -> Void)?

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        done(player)
    }

    func audioPlayerDecodeErrorDidOccur(_ player: AVAudioPlayer, error: Error?) {
        done(player)
    }

    private func done(_ player: AVAudioPlayer) {
        let id = ObjectIdentifier(player)
        Task { @MainActor [finished] in finished?(id) }
    }
}
