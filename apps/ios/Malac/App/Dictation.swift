import AVFoundation
import MalacKit
import Observation

/// Records what the user says as 16 kHz mono 16-bit audio and has the
/// daemon's dictation model turn it into text.
@MainActor
@Observable
final class Dictation {
    enum Phase: Equatable {
        case idle
        case recording(since: Date)
        case transcribing
    }

    private(set) var phase = Phase.idle
    private(set) var levels = LevelHistory()

    /// The text heard, for the composer to insert.
    @ObservationIgnored var onText: (String) -> Void = { _ in }
    @ObservationIgnored var onError: (String) -> Void = { _ in }

    /// The daemon takes up to 25 MB, about 13 minutes; stop well before.
    static let longest: Duration = .seconds(600)

    @ObservationIgnored private var engine: AVAudioEngine?
    @ObservationIgnored private let recorded = SampleBuffer()
    @ObservationIgnored private var api: DaemonAPI?
    @ObservationIgnored private var speech: SpeechPlayer?
    @ObservationIgnored private var watching: Task<Void, Never>?
    @ObservationIgnored private var transcribing: Task<Void, Never>?

    var isRecording: Bool {
        if case .recording = phase { return true }
        return false
    }

    /// Asks for the microphone the first time, then records until `finish` or `cancel`.
    func start(api: DaemonAPI, speech: SpeechPlayer) async {
        guard phase == .idle else { return }
        do {
            try await Microphone.authorize()
            speech.paused = true
            let session = AVAudioSession.sharedInstance()
            try session.setCategory(.playAndRecord, mode: .default, options: [.duckOthers, .defaultToSpeaker, .allowBluetooth])
            try session.setActive(true)
            let engine = AVAudioEngine()
            let recorded = recorded
            _ = recorded.take()
            try Microphone.tap(engine.inputNode, rate: PCM16.dictationRate) { [weak self] samples in
                recorded.append(samples)
                let level = PCM16.level(samples)
                Task { @MainActor [weak self] in self?.levels.add(level) }
            }
            engine.prepare()
            do {
                try engine.start()
            } catch {
                engine.inputNode.removeTap(onBus: 0)
                throw Microphone.Failure.unavailable(error.localizedDescription)
            }
            self.engine = engine
            self.api = api
            self.speech = speech
        } catch {
            speech.paused = false
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
            onError(error.localizedDescription)
            return
        }
        phase = .recording(since: Date())
        // A call or Siri takes the microphone, and a long recording stops on its own: send what there is.
        watching = Task { [weak self] in
            await withTaskGroup(of: Void.self) { group in
                group.addTask {
                    for await _ in NotificationCenter.default.notifications(named: AVAudioSession.interruptionNotification) { return }
                }
                group.addTask { try? await Task.sleep(for: Self.longest) }
                await group.next()
                group.cancelAll()
            }
            guard !Task.isCancelled else { return }
            self?.finish()
        }
    }

    /// Stops recording and transcribes it.
    func finish() {
        guard isRecording, let api else { return }
        let samples = stopRecording()
        guard samples.count >= PCM16.dictationRate / 2 else {
            phase = .idle
            onError("That was too short to transcribe. Speak for a moment before you stop.")
            return
        }
        phase = .transcribing
        let wav = WAV.encode(samples, rate: PCM16.dictationRate)
        transcribing = Task { [weak self] in
            do {
                let heard = try await api.transcribe(wav: wav)
                guard let self, !Task.isCancelled else { return }
                self.phase = .idle
                let text = heard.text.trimmingCharacters(in: .whitespacesAndNewlines)
                if text.isEmpty {
                    self.onError("Nothing was heard. Speak a little closer to the microphone and try again.")
                } else {
                    self.onText(text)
                }
            } catch {
                guard let self, !Task.isCancelled else { return }
                self.phase = .idle
                self.onError("Dictation failed: \(error.localizedDescription)")
            }
        }
    }

    /// Throws the recording away, or stops waiting for its text.
    func cancel() {
        transcribing?.cancel()
        transcribing = nil
        _ = stopRecording()
        phase = .idle
    }

    private func stopRecording() -> [Int16] {
        watching?.cancel()
        watching = nil
        if let engine {
            engine.inputNode.removeTap(onBus: 0)
            engine.stop()
            self.engine = nil
            try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        }
        speech?.paused = false
        speech = nil
        levels.reset()
        return recorded.take()
    }
}
