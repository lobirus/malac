import Foundation

/// 16-bit little-endian PCM, the format dictation records (16 kHz mono) and
/// talk mode streams both ways (24 kHz mono).
public enum PCM16 {
    /// Dictation's rate: every transcription model takes 16 kHz mono WAV.
    public static let dictationRate = 16_000
    /// Talk mode's rate, both ways.
    public static let talkRate = 24_000

    /// Samples as little-endian bytes.
    public static func data(_ samples: [Int16]) -> Data {
        var data = Data(capacity: samples.count * 2)
        for sample in samples {
            let value = UInt16(bitPattern: sample.littleEndian)
            data.append(UInt8(value & 0xFF))
            data.append(UInt8(value >> 8))
        }
        return data
    }

    /// Little-endian bytes as samples; an odd last byte is dropped.
    public static func samples(_ data: Data) -> [Int16] {
        data.withUnsafeBytes { raw in
            (0 ..< raw.count / 2).map { index in
                Int16(bitPattern: UInt16(raw[index * 2]) | UInt16(raw[index * 2 + 1]) << 8)
            }
        }
    }

    /// Float samples in -1…1, clamped, as 16-bit samples.
    public static func samples(_ floats: some Collection<Float>) -> [Int16] {
        floats.map { Int16((max(-1, min(1, $0)) * Float(Int16.max)).rounded()) }
    }

    /// Little-endian bytes as float samples in -1…1, for playback.
    public static func floats(_ data: Data) -> [Float] {
        samples(data).map { Float($0) / Float(Int16.max) }
    }

    /// A meter value from 0 (silence, -50 dBFS or less) to 1 (full scale), from the RMS.
    public static func level(_ samples: some Collection<Int16>) -> Float {
        guard !samples.isEmpty else { return 0 }
        let sum = samples.reduce(0.0) { total, sample in
            let value = Double(sample) / Double(Int16.max)
            return total + value * value
        }
        let rms = (sum / Double(samples.count)).squareRoot()
        guard rms > 0 else { return 0 }
        return Float(max(0, min(1, (20 * log10(rms) + 50) / 50)))
    }

    /// How long `bytes` of mono audio at `rate` play.
    public static func duration(bytes: Int, rate: Int) -> TimeInterval {
        Double(bytes / 2) / Double(rate)
    }
}

/// WAV files of 16-bit mono PCM, with the plain 44-byte header every model reads.
public enum WAV {
    public static func encode(_ samples: [Int16], rate: Int) -> Data {
        encode(pcm: PCM16.data(samples), rate: rate)
    }

    /// `pcm` is 16-bit little-endian mono.
    public static func encode(pcm: Data, rate: Int) -> Data {
        let channels = 1
        let bits = 16
        var data = Data(capacity: 44 + pcm.count)
        func text(_ value: String) { data.append(contentsOf: Array(value.utf8)) }
        func u32(_ value: Int) { withUnsafeBytes(of: UInt32(value).littleEndian) { data.append(contentsOf: $0) } }
        func u16(_ value: Int) { withUnsafeBytes(of: UInt16(value).littleEndian) { data.append(contentsOf: $0) } }
        text("RIFF")
        u32(36 + pcm.count)
        text("WAVE")
        text("fmt ")
        u32(16)
        u16(1) // PCM
        u16(channels)
        u32(rate)
        u32(rate * channels * bits / 8)
        u16(channels * bits / 8)
        u16(bits)
        text("data")
        u32(pcm.count)
        data.append(pcm)
        return data
    }

    /// The rate and samples of a 16-bit mono PCM WAV, skipping chunks other than `fmt ` and `data`.
    public static func decode(_ wav: Data) -> (rate: Int, samples: [Int16])? {
        let bytes = [UInt8](wav)
        func u32(_ at: Int) -> Int { Int(bytes[at]) | Int(bytes[at + 1]) << 8 | Int(bytes[at + 2]) << 16 | Int(bytes[at + 3]) << 24 }
        func u16(_ at: Int) -> Int { Int(bytes[at]) | Int(bytes[at + 1]) << 8 }
        guard bytes.count >= 12, bytes[0 ..< 4].elementsEqual("RIFF".utf8), bytes[8 ..< 12].elementsEqual("WAVE".utf8) else { return nil }
        var offset = 12
        var rate: Int?
        while offset + 8 <= bytes.count {
            let size = u32(offset + 4)
            let body = offset + 8
            if bytes[offset ..< offset + 4].elementsEqual("fmt ".utf8), body + 16 <= bytes.count {
                guard u16(body) == 1, u16(body + 2) == 1, u16(body + 14) == 16 else { return nil }
                rate = u32(body + 4)
            } else if bytes[offset ..< offset + 4].elementsEqual("data".utf8), let rate {
                let end = min(bytes.count, body + size)
                return (rate, PCM16.samples(Data(bytes[body ..< end])))
            }
            offset = body + size + size % 2
        }
        return nil
    }
}

/// Cuts a stream of PCM bytes into frames of one size, as talk mode sends
/// them: about 100 ms each.
public struct PCMFrames: Sendable {
    public let frameBytes: Int
    private var pending = Data()

    public init(rate: Int = PCM16.talkRate, milliseconds: Int = 100) {
        frameBytes = rate * milliseconds / 1000 * 2
    }

    /// Adds audio and returns the whole frames it completes.
    public mutating func append(_ data: Data) -> [Data] {
        pending.append(data)
        var frames: [Data] = []
        while pending.count >= frameBytes {
            frames.append(Data(pending.prefix(frameBytes)))
            pending = Data(pending.dropFirst(frameBytes))
        }
        return frames
    }

    /// What is left, shorter than a frame.
    public mutating func flush() -> Data? {
        defer { pending = Data() }
        return pending.isEmpty ? nil : pending
    }
}
