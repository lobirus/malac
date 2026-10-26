import Foundation

/// A one-time pairing code from `malac remote pair`: ten characters of
/// Crockford's base32, shown as `ABCDE-FGHJK`.
public enum PairingCode {
    public static let alphabet = Set("0123456789ABCDEFGHJKMNPQRSTVWXYZ")
    public static let length = 10

    /// Letters and digits only, upper case, with the look-alikes O, I and L read as 0, 1 and 1.
    public static func normalize(_ code: String) -> String {
        String(code.filter { $0.isASCII && ($0.isLetter || $0.isNumber) }.uppercased().map { character -> Character in
            switch character {
            case "O": "0"
            case "I", "L": "1"
            default: character
            }
        })
    }

    public static func isValid(_ code: String) -> Bool {
        let normalized = normalize(code)
        return normalized.count == length && normalized.allSatisfy(alphabet.contains)
    }

    public static func format(_ code: String) -> String {
        let normalized = normalize(code)
        guard normalized.count == length else { return normalized }
        return "\(normalized.prefix(5))-\(normalized.dropFirst(5))"
    }
}

/// What a QR code or a pasted link says about a daemon: its address and a
/// pairing code (`<base>/#pair=CODE`) or a token (`<base>/?token=…`).
public struct PairingLink: Hashable, Sendable {
    /// The daemon's address; a path is kept, for a daemon behind a reverse proxy.
    public var base: URL
    public var code: String?
    public var token: String?

    public init(base: URL, code: String? = nil, token: String? = nil) {
        self.base = base
        self.code = code
        self.token = token
    }

    public static func parse(_ text: String) -> PairingLink? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let withScheme = trimmed.contains("://") ? trimmed : "http://\(trimmed)"
        guard var components = URLComponents(string: withScheme),
              let scheme = components.scheme?.lowercased(), ["http", "https"].contains(scheme),
              let host = components.host, !host.isEmpty
        else { return nil }
        let code = (components.fragment ?? "").split(separator: "&")
            .first { $0.hasPrefix("pair=") }
            .map { PairingCode.format(String($0.dropFirst(5))) }
            .flatMap { $0.isEmpty ? nil : $0 }
        let token = components.queryItems?.first { $0.name == "token" }?.value.flatMap { $0.isEmpty ? nil : $0 }
        components.scheme = scheme
        components.query = nil
        components.fragment = nil
        if !components.path.hasSuffix("/") { components.path += "/" }
        guard let base = components.url else { return nil }
        return PairingLink(base: base, code: code, token: token)
    }
}

/// Trades a pairing code for a device token. Behind a protocol so pairing can
/// follow the daemon as it changes, and be faked in previews and tests.
public protocol PairingService: Sendable {
    func hello(_ base: URL) async throws -> Hello
    func pair(_ base: URL, code: String, deviceName: String) async throws -> Paired
}

/// Pairing over the daemon's public endpoints, which need no token.
public struct DaemonPairing: PairingService {
    private let session: URLSession

    public init(session: URLSession = .shared) {
        self.session = session
    }

    public func hello(_ base: URL) async throws -> Hello {
        let request = URLRequest(url: base.appending(path: "api/hello"), timeoutInterval: 10)
        return try await HTTP.send(request, session: session, as: Hello.self)
    }

    public func pair(_ base: URL, code: String, deviceName: String) async throws -> Paired {
        var request = URLRequest(url: base.appending(path: "api/pair"), timeoutInterval: 15)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body = ["code": PairingCode.format(code), "name": deviceName, "kind": "app"]
        request.httpBody = try JSONEncoder().encode(body)
        return try await HTTP.send(request, session: session, as: Paired.self)
    }
}
