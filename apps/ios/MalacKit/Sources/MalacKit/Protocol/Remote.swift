import Foundation

/// When the daemon keeps its machine from sleeping.
public enum KeepAwake: String, LenientEnum, CaseIterable, Identifiable {
    /// While agents or jobs run, and a minute after.
    case busy
    case always
    case off
    public static let fallback = KeepAwake.busy

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .busy: "While working"
        case .always: "Always"
        case .off: "Off"
        }
    }
}

/// Whether the daemon holds a power assertion, and why.
public struct AwakeStatus: Hashable, Sendable, Decodable {
    public var mode: KeepAwake
    public var awake: Bool
    /// For people: "2 agents running", "nothing is running".
    public var reason: String
    /// `caffeinate` or `systemd-inhibit`.
    public var method: String?
    public var error: String?
    public var since: Date?

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        mode = c.value("mode", default: .busy)
        awake = c.value("awake", default: false)
        reason = c.value("reason", default: "")
        method = c.optional("method")
        error = c.optional("error")
        since = c.optional("since")
    }
}

/// How other machines reach the daemon.
public struct RemoteStatus: Hashable, Sendable, Decodable {
    public struct Address: Hashable, Sendable, Decodable {
        /// `tunnel`, `public`, `lan` or `local`.
        public var kind: String
        public var url: String

        public init(from decoder: Decoder) throws {
            let c = try decoder.fields()
            kind = c.value("kind", default: "")
            url = c.value("url", default: "")
        }
    }

    public struct Tunnel: Hashable, Sendable, Decodable {
        /// `cloudflare`, `ngrok`, `tailscale` or `tailscale-funnel`.
        public var provider: String
        /// `starting`, `running`, `failed` or `stopped`.
        public var state: String
        public var url: String?
        public var error: String?

        public init(from decoder: Decoder) throws {
            let c = try decoder.fields()
            provider = c.value("provider", default: "")
            state = c.value("state", default: "")
            url = c.optional("url")
            error = c.optional("error")
        }
    }

    /// Paired devices on other machines may connect.
    public var enabled: Bool
    /// The bound address, e.g. `0.0.0.0:7700`.
    public var listen: String
    /// Addresses other devices can use, best first.
    public var urls: [Address]
    public var tunnel: Tunnel?
    /// Paired devices.
    public var devices: Int

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        enabled = c.value("enabled", default: false)
        listen = c.value("listen", default: "")
        urls = c.value("urls", default: [])
        tunnel = c.optional("tunnel")
        devices = c.value("devices", default: 0)
    }
}

/// `GET /api/hello`: says a malac daemon answers at an address, before pairing.
public struct Hello: Hashable, Sendable, Decodable {
    public var app: String
    public var version: String
    /// Whether the daemon accepts remote devices.
    public var remote: Bool
    public var pairing: Bool

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        app = c.value("app", default: "")
        version = c.value("version", default: "")
        remote = c.value("remote", default: false)
        pairing = c.value("pairing", default: false)
    }
}

/// A paired device, as `GET /api/devices` lists it.
public struct DeviceView: Identifiable, Hashable, Sendable, Decodable {
    public var id: String
    public var name: String
    /// `cli`, `browser`, `app` or what the device said it is.
    public var kind: String
    public var created: Date?
    public var lastSeen: Date?
    /// Where it last connected from, as the daemon saw it.
    public var address: String?
    /// The device making the request.
    public var current: Bool

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        id = try c.value("id")
        name = c.value("name", default: "")
        kind = c.value("kind", default: "")
        created = c.optional("created")
        lastSeen = c.optional("last_seen")
        address = c.optional("address")
        current = c.value("current", default: false)
    }
}

/// `POST /api/pair`: the device token the daemon issued for this phone.
public struct Paired: Sendable, Decodable {
    public var token: String
    public var device: DeviceView?
    /// The daemon's version.
    public var version: String

    public init(from decoder: Decoder) throws {
        let c = try decoder.fields()
        token = try c.value("token")
        device = c.optional("device")
        version = c.value("version", default: "")
    }
}
