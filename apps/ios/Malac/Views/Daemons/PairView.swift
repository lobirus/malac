import MalacKit
import SwiftUI

/// Pairs this phone with a daemon: scan the QR code from `malac remote pair`,
/// or type its address and the code. A token from elsewhere works too.
struct PairView: View {
    enum Method: Hashable { case code, token }

    /// A daemon that refused its token and is paired again.
    var again: SavedDaemon?

    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var address = ""
    @State private var method = Method.code
    @State private var code = ""
    @State private var token = ""
    @State private var deviceName = UIDevice.current.name
    @State private var name = ""
    @State private var scanning = false
    @State private var working = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Button {
                        scanning = true
                    } label: {
                        Label("Scan the pairing code", systemImage: "qrcode.viewfinder")
                    }
                } footer: {
                    Text("Run `malac remote pair` on the daemon's machine. Codes are valid for ten minutes and one device.")
                }

                Section("Daemon") {
                    TextField("http://192.168.1.20:7700", text: $address)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityLabel("Daemon address")
                    Picker("Pair with", selection: $method) {
                        Text("Code").tag(Method.code)
                        Text("Token").tag(Method.token)
                    }
                    .pickerStyle(.segmented)
                    if method == .code {
                        TextField("ABCDE-FGHJK", text: $code)
                            .textInputAutocapitalization(.characters)
                            .autocorrectionDisabled()
                            .font(.body.monospaced())
                            .accessibilityLabel("Pairing code")
                    } else {
                        SecureField("Device token", text: $token)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    }
                }

                Section {
                    TextField("Name in this app", text: $name)
                    if method == .code {
                        TextField("This phone's name on the daemon", text: $deviceName)
                    }
                } footer: {
                    Text("The daemon lists this phone under its name in `malac remote devices`, where it can be revoked.")
                }

                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(Palette.red)
                    }
                }
            }
            .malacList()
            .navigationTitle(again == nil ? "Pair a daemon" : "Pair again")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if working {
                        ProgressView()
                    } else {
                        Button("Pair") { Task { await pair() } }.disabled(!canPair)
                    }
                }
            }
            .sheet(isPresented: $scanning) {
                QRScannerSheet { scanned in
                    scanning = false
                    use(scanned)
                }
            }
            .onAppear {
                if let again {
                    address = again.base.absoluteString
                    name = again.name
                }
            }
        }
    }

    private var canPair: Bool {
        guard PairingLink.parse(address) != nil else { return false }
        return method == .code ? PairingCode.isValid(code) : !token.isEmpty
    }

    private func use(_ text: String) {
        guard let link = PairingLink.parse(text) else {
            error = "That code is not a malac pairing link."
            return
        }
        address = link.base.absoluteString
        if let code = link.code {
            method = .code
            self.code = code
        } else if let token = link.token {
            method = .token
            self.token = token
        }
        error = nil
        if canPair { Task { await pair() } }
    }

    private func pair() async {
        guard let link = PairingLink.parse(address) else { return }
        working = true
        error = nil
        defer { working = false }
        do {
            let hello = try await app.pairing.hello(link.base)
            guard hello.app == "malac" else { throw PairingFailure.notMalac }
            let label = name.trimmingCharacters(in: .whitespaces).isEmpty ? link.base.host() ?? "daemon" : name
            switch method {
            case .code:
                let paired = try await app.pairing.pair(link.base, code: code, deviceName: deviceName)
                try app.add(base: link.base, name: label, token: paired.token, deviceId: paired.device?.id)
            case .token:
                // A token from elsewhere, e.g. one an SSH tunnel makes local: check it before saving.
                _ = try await DaemonAPI(endpoint: DaemonEndpoint(base: link.base, token: token)).snapshot()
                try app.add(base: link.base, name: label, token: token, deviceId: nil)
            }
            dismiss()
        } catch let failure as DaemonError {
            error = Self.message(for: failure)
        } catch {
            self.error = (error as? PairingFailure)?.errorDescription ?? error.localizedDescription
        }
    }

    private static func message(for failure: DaemonError) -> String {
        switch failure {
        case .forbidden(let message) where !message.isEmpty:
            return message.prefix(1).uppercased() + message.dropFirst() + "."
        case .forbidden:
            return "The daemon refused the code."
        case .http(429, _):
            return "Too many wrong codes. Create a new one with `malac remote pair`."
        case .unauthorized:
            return "The daemon did not accept that token."
        default:
            return failure.localizedDescription
        }
    }

    private enum PairingFailure: LocalizedError {
        case notMalac
        var errorDescription: String? { "No malac daemon answers at that address." }
    }
}
