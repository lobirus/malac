import Foundation

/// Timestamps as the daemon writes them: RFC 3339 in UTC or with an offset,
/// with up to nine fractional digits (`2026-10-26T06:51:44.434082473Z`).
public enum Timestamp {
    public static func parse(_ text: String) -> Date? {
        let scalars = Array(text.utf8)
        func number(_ start: Int, _ count: Int) -> Int? {
            guard start + count <= scalars.count else { return nil }
            var value = 0
            for byte in scalars[start ..< start + count] {
                guard (48 ... 57).contains(byte) else { return nil }
                value = value * 10 + Int(byte - 48)
            }
            return value
        }
        guard scalars.count >= 20,
              let year = number(0, 4), let month = number(5, 2), let day = number(8, 2),
              let hour = number(11, 2), let minute = number(14, 2), let second = number(17, 2),
              scalars[4] == UInt8(ascii: "-"), scalars[7] == UInt8(ascii: "-"),
              [UInt8(ascii: "T"), UInt8(ascii: "t"), UInt8(ascii: " ")].contains(scalars[10])
        else { return nil }

        var index = 19
        var fraction = 0.0
        if index < scalars.count, scalars[index] == UInt8(ascii: ".") {
            index += 1
            var scale = 0.1
            while index < scalars.count, (48 ... 57).contains(scalars[index]) {
                fraction += Double(scalars[index] - 48) * scale
                scale /= 10
                index += 1
            }
        }
        guard index < scalars.count else { return nil }
        var offset = 0
        switch scalars[index] {
        case UInt8(ascii: "Z"), UInt8(ascii: "z"):
            guard index + 1 == scalars.count else { return nil }
        case UInt8(ascii: "+"), UInt8(ascii: "-"):
            guard index + 6 == scalars.count, let hours = number(index + 1, 2), let minutes = number(index + 4, 2) else { return nil }
            offset = (hours * 3600 + minutes * 60) * (scalars[index] == UInt8(ascii: "-") ? -1 : 1)
        default:
            return nil
        }

        var components = DateComponents()
        components.year = year
        components.month = month
        components.day = day
        components.hour = hour
        components.minute = minute
        components.second = second
        guard let whole = calendar.date(from: components) else { return nil }
        return whole.addingTimeInterval(fraction - Double(offset))
    }

    private static let calendar: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }()
}

public extension JSONDecoder {
    /// A decoder for daemon messages.
    static func malac() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let text = try container.decode(String.self)
            guard let date = Timestamp.parse(text) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "not an RFC 3339 timestamp: \(text)")
            }
            return date
        }
        return decoder
    }
}

/// A string-backed enum that decodes values it does not know as `fallback`,
/// so a newer daemon never breaks an older app.
public protocol LenientEnum: RawRepresentable, Codable, Hashable, Sendable where RawValue == String {
    static var fallback: Self { get }
}

public extension LenientEnum {
    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? .fallback
    }
}

/// Coding keys made at run time, for the `type`-tagged messages.
struct AnyKey: CodingKey {
    let stringValue: String
    var intValue: Int? { nil }

    init(_ string: String) { stringValue = string }
    init?(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
}

extension KeyedDecodingContainer where Key == AnyKey {
    /// A required field.
    func value<T: Decodable>(_ key: String) throws -> T {
        try decode(T.self, forKey: AnyKey(key))
    }

    /// An optional field: missing, null or (for leniency) malformed reads as nil.
    func optional<T: Decodable>(_ key: String) -> T? {
        (try? decodeIfPresent(T.self, forKey: AnyKey(key))) ?? nil
    }

    /// A field with a default, for what older or newer daemons may leave out.
    func value<T: Decodable>(_ key: String, default fallback: T) -> T {
        optional(key) ?? fallback
    }
}

extension Decoder {
    func fields() throws -> KeyedDecodingContainer<AnyKey> {
        try container(keyedBy: AnyKey.self)
    }
}

extension KeyedEncodingContainer where Key == AnyKey {
    mutating func put<T: Encodable>(_ value: T, _ key: String) throws {
        try encode(value, forKey: AnyKey(key))
    }

    mutating func put<T: Encodable>(ifPresent value: T?, _ key: String) throws {
        try encodeIfPresent(value, forKey: AnyKey(key))
    }
}
