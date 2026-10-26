import Foundation
import Security

/// Device tokens, one generic password per saved daemon. Readable after the
/// first unlock so background refresh works on a locked phone, and never
/// copied to another device by backups.
struct Keychain {
    enum Failure: LocalizedError {
        case status(OSStatus)

        var errorDescription: String? {
            switch self {
            case .status(let status): SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error \(status)"
            }
        }
    }

    var service = "io.malac.app.device-token"

    func token(for id: UUID) -> String? {
        var query = base(id)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    func set(_ token: String, for id: UUID) throws {
        let data = Data(token.utf8)
        let update = [kSecValueData as String: data]
        var status = SecItemUpdate(base(id) as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            var item = base(id)
            item[kSecValueData as String] = data
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(item as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw Failure.status(status) }
    }

    func remove(for id: UUID) {
        SecItemDelete(base(id) as CFDictionary)
    }

    private func base(_ id: UUID) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: id.uuidString,
        ]
    }
}
