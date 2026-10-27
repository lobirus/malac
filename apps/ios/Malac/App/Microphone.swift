import AVFoundation
import MalacKit
import UIKit

/// Microphone permission and the conversion from the input node's format
/// to the 16-bit mono PCM the daemon takes.
enum Microphone {
    enum Failure: LocalizedError {
        case denied
        case unavailable(String)

        var errorDescription: String? {
            switch self {
            case .denied: "Microphone access is off for malac. Turn it on in Settings → Privacy & Security → Microphone."
            case .unavailable(let reason): "The microphone cannot be used: \(reason)"
            }
        }
    }

    /// Asks the first time; throws when the user said no.
    static func authorize() async throws {
        switch AVAudioApplication.shared.recordPermission {
        case .granted:
            return
        case .denied:
            throw Failure.denied
        default:
            guard await AVAudioApplication.requestRecordPermission() else { throw Failure.denied }
        }
    }

    @MainActor
    static func openSettings() {
        if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
    }

    /// Taps `input` and hands its audio, as 16-bit mono at `rate`, to
    /// `handler` on the audio thread.
    static func tap(_ input: AVAudioInputNode, rate: Int, handler: @escaping @Sendable ([Int16]) -> Void) throws {
        let format = input.outputFormat(forBus: 0)
        guard format.channelCount > 0, let converter = PCMConverter(from: format, rate: rate) else {
            throw Failure.unavailable("this device has no audio input.")
        }
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, _ in
            handler(converter.convert(buffer))
        }
    }
}

/// Samples gathered on the audio thread, taken on the main thread.
final class SampleBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var samples: [Int16] = []

    func append(_ new: [Int16]) {
        lock.withLock { samples += new }
    }

    func take() -> [Int16] {
        lock.withLock {
            defer { samples = [] }
            return samples
        }
    }
}

/// Converts the microphone's buffers to 16-bit mono at one rate. Called on
/// the tap's thread only; the converter keeps its state between buffers.
final class PCMConverter {
    private let converter: AVAudioConverter
    private let output: AVAudioFormat
    private let ratio: Double

    init?(from input: AVAudioFormat, rate: Int) {
        guard input.sampleRate > 0,
              let output = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(rate), channels: 1, interleaved: true),
              let converter = AVAudioConverter(from: input, to: output) else { return nil }
        self.converter = converter
        self.output = output
        ratio = Double(rate) / input.sampleRate
    }

    func convert(_ buffer: AVAudioPCMBuffer) -> [Int16] {
        let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up)) + 16
        guard let converted = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: capacity) else { return [] }
        var fed = false
        var error: NSError?
        converter.convert(to: converted, error: &error) { _, status in
            if fed {
                status.pointee = .noDataNow
                return nil
            }
            fed = true
            status.pointee = .haveData
            return buffer
        }
        guard error == nil, let channel = converted.int16ChannelData else { return [] }
        return Array(UnsafeBufferPointer(start: channel[0], count: Int(converted.frameLength)))
    }
}

/// Recent meter values, for the little level display while recording or talking.
struct LevelHistory: Equatable {
    static let count = 16
    private(set) var values = [Float](repeating: 0, count: count)

    mutating func add(_ level: Float) {
        values.removeFirst()
        values.append(level)
    }

    mutating func reset() {
        values = [Float](repeating: 0, count: Self.count)
    }
}
