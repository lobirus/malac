import AVFoundation
import MalacKit
import Observation

/// Talk mode: the microphone streams to the daemon's voice model over
/// `/api/voice/talk` once it is ready, and the voice plays back. Voice
/// processing cancels the speaker's echo, so the voice does not hear itself.
@MainActor
@Observable
final class TalkModel {
    private(set) var state = TalkState()
    private(set) var levels = LevelHistory()
    private(set) var muted = false
    /// The microphone was refused; the view offers Settings.
    private(set) var needsPermission = false

    @ObservationIgnored private let connection: TalkConnection
    @ObservationIgnored private let audio = TalkAudio()
    @ObservationIgnored private let speech: SpeechPlayer
    @ObservationIgnored private var started = false
    @ObservationIgnored private var watching: Task<Void, Never>?

    init(endpoint: DaemonEndpoint, tab: String?, agent: String?, speech: SpeechPlayer) {
        connection = TalkConnection(endpoint: endpoint, tab: tab, agent: agent)
        self.speech = speech
    }

    var isEnded: Bool { state.phase == .ended }

    func start() async {
        guard !started else { return }
        started = true
        do {
            try await Microphone.authorize()
        } catch {
            needsPermission = true
            state.end(error.localizedDescription)
            return
        }
        guard !isEnded else { return }
        // The daemon's sentences would talk over the voice; it tells the user itself.
        speech.paused = true
        audio.onFrame = { [weak self] frame in self?.microphone(frame) }
        audio.onLevel = { [weak self] level in
            guard let self else { return }
            self.levels.add(self.muted ? 0 : level)
        }
        audio.onPlaying = { [weak self] playing in self?.state.playing = playing }
        audio.onFailure = { [weak self] message in self?.end(message) }
        do {
            try audio.start()
        } catch {
            finish("The microphone cannot be used: \(error.localizedDescription)")
            return
        }
        // A call or Siri takes the microphone and the speaker.
        watching = Task { [weak self] in
            for await _ in NotificationCenter.default.notifications(named: AVAudioSession.interruptionNotification).map({ _ in }) {
                self?.end("Talk mode ended because another app took the microphone.")
                return
            }
        }
        connection.onEvent = { [weak self] event in self?.received(event) }
        connection.onAudio = { [weak self] pcm in self?.audio.play(pcm) }
        connection.start()
    }

    /// Ends the conversation; the daemon hears `stop`.
    func end(_ reason: String = "You ended talk mode.") {
        guard !isEnded else { return }
        connection.close()
        finish(reason)
    }

    func toggleMute() {
        muted.toggle()
        audio.muted = muted
        if muted { levels.reset() }
    }

    /// Stops the voice's reply.
    func interrupt() {
        audio.stopPlayback()
        connection.send(.interrupt)
    }

    /// Typed input, as if spoken.
    func send(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, state.ready, !isEnded else { return }
        state.typed(text)
        connection.send(.text(text))
    }

    private func microphone(_ frame: Data) {
        // Audio goes out only once the voice model is ready.
        guard state.ready, !muted, !isEnded else { return }
        connection.send(audio: frame)
    }

    private func received(_ event: TalkEvent) {
        state.apply(event)
        switch event {
        case .listening:
            // The user started speaking: the voice stops at once.
            audio.stopPlayback()
        case .closed:
            teardown()
        default:
            break
        }
    }

    private func finish(_ reason: String) {
        state.end(reason)
        teardown()
    }

    private func teardown() {
        watching?.cancel()
        watching = nil
        audio.stop()
        speech.paused = false
    }
}

/// The audio engine of talk mode: the microphone through voice processing,
/// as 24 kHz 16-bit frames of 100 ms, and a player node for the voice's
/// 24 kHz audio on the same engine, which voice processing needs to cancel it.
@MainActor
final class TalkAudio {
    var onFrame: ((Data) -> Void)?
    var onLevel: ((Float) -> Void)?
    var onPlaying: ((Bool) -> Void)?
    /// The engine stopped and could not start again.
    var onFailure: ((String) -> Void)?
    var muted = false {
        didSet { if running { engine.inputNode.isVoiceProcessingInputMuted = muted } }
    }

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(PCM16.talkRate), channels: 1, interleaved: false)!
    private var running = false
    /// Buffers scheduled and not played yet, of the current `generation`.
    private var scheduled = 0
    private var generation = 0
    private var watching: Task<Void, Never>?

    func start() throws {
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetooth])
        try session.setActive(true)
        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(true)
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: format)
        do {
            try tap()
            engine.prepare()
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            try? input.setVoiceProcessingEnabled(false)
            try? session.setActive(false, options: .notifyOthersOnDeactivation)
            throw error
        }
        running = true
        player.play()
        // Headphones coming or going stop the engine; start it again with the new input.
        let id = ObjectIdentifier(engine)
        let changes = NotificationCenter.default.notifications(named: .AVAudioEngineConfigurationChange)
            .filter { ($0.object as AnyObject?).map(ObjectIdentifier.init) == id }
            .map { _ in }
        watching = Task { [weak self] in
            for await _ in changes {
                self?.restart()
            }
        }
    }

    private func tap() throws {
        let frames = FrameQueue()
        try Microphone.tap(engine.inputNode, rate: PCM16.talkRate) { [weak self] samples in
            let ready = frames.append(PCM16.data(samples))
            let level = PCM16.level(samples)
            Task { @MainActor [weak self] in
                guard let self, self.running else { return }
                for frame in ready { self.onFrame?(frame) }
                self.onLevel?(level)
            }
        }
    }

    private func restart() {
        guard running, !engine.isRunning else { return }
        stopPlayback()
        engine.inputNode.removeTap(onBus: 0)
        do {
            try tap()
            engine.prepare()
            try engine.start()
            player.play()
        } catch {
            onFailure?("The audio stopped and could not start again: \(error.localizedDescription)")
        }
    }

    /// Queues the voice's audio: 16-bit little-endian mono at 24 kHz.
    func play(_ pcm: Data) {
        // A player node raises an exception when it plays on a stopped engine.
        guard running, engine.isRunning else { return }
        let samples = PCM16.floats(pcm)
        guard !samples.isEmpty, let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
              let channel = buffer.floatChannelData else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { channel[0].update(from: $0.baseAddress!, count: samples.count) }
        let batch = generation
        scheduled += 1
        if scheduled == 1 { onPlaying?(true) }
        Self.schedule(buffer, on: player) { [weak self] in
            Task { @MainActor [weak self] in self?.played(batch) }
        }
        if !player.isPlaying { player.play() }
    }

    /// Silences the voice at once, dropping what is queued.
    func stopPlayback() {
        generation += 1
        let wasPlaying = scheduled > 0
        scheduled = 0
        player.stop()
        if running, engine.isRunning { player.play() }
        if wasPlaying { onPlaying?(false) }
    }

    func stop() {
        guard running else { return }
        running = false
        watching?.cancel()
        watching = nil
        generation += 1
        scheduled = 0
        engine.inputNode.removeTap(onBus: 0)
        player.stop()
        engine.stop()
        try? engine.inputNode.setVoiceProcessingEnabled(false)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        onPlaying?(false)
    }

    private func played(_ batch: Int) {
        guard batch == generation, scheduled > 0 else { return }
        scheduled -= 1
        if scheduled == 0 { onPlaying?(false) }
    }

    // Outside the main actor: the player calls back on its own thread.
    private nonisolated static func schedule(_ buffer: AVAudioPCMBuffer, on player: AVAudioPlayerNode, played: @escaping @Sendable () -> Void) {
        player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { _ in played() }
    }
}

/// Cuts the microphone's audio into 100 ms frames on the audio thread.
private final class FrameQueue: @unchecked Sendable {
    private let lock = NSLock()
    private var frames = PCMFrames()

    func append(_ data: Data) -> [Data] {
        lock.withLock { frames.append(data) }
    }
}
