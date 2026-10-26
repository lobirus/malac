import Foundation
import MalacKit
import UserNotifications

/// Local notifications for approvals and finished turns, posted while the
/// app is in the background. Approval notifications can be answered from
/// the notification itself.
@MainActor
final class Notifier {
    enum Action: String {
        case allow = "ALLOW"
        case always = "ALWAYS"
        case deny = "DENY"

        var decision: Decision {
            switch self {
            case .allow: .allow
            case .always: .allowSession
            case .deny: .deny
            }
        }
    }

    /// What a notification carries, to act on it later.
    struct Target {
        var daemon: UUID
        var agent: String?
        var request: String?
    }

    static let approvalCategory = "approval"

    private let center = UNUserNotificationCenter.current()

    func registerCategories() {
        let approval = UNNotificationCategory(
            identifier: Self.approvalCategory,
            actions: [
                UNNotificationAction(identifier: Action.allow.rawValue, title: "Allow", options: [.authenticationRequired]),
                UNNotificationAction(identifier: Action.always.rawValue, title: "Always allow", options: [.authenticationRequired]),
                UNNotificationAction(identifier: Action.deny.rawValue, title: "Deny", options: [.destructive]),
            ],
            intentIdentifiers: []
        )
        center.setNotificationCategories([approval])
    }

    @discardableResult
    func requestAuthorization() async -> Bool {
        (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
    }

    func authorizationStatus() async -> UNAuthorizationStatus {
        await center.notificationSettings().authorizationStatus
    }

    func approval(_ request: PendingApproval, place: String, daemon: SavedDaemon) {
        let content = UNMutableNotificationContent()
        content.title = "\(request.agentLabel) needs approval"
        content.subtitle = place.isEmpty ? daemon.name : "\(place) · \(daemon.name)"
        content.body = "\(ToolPresentation.title(request.tool).title): \(Format.truncate(request.summary, 200))"
        content.categoryIdentifier = Self.approvalCategory
        content.threadIdentifier = request.agent
        content.sound = .default
        content.userInfo = ["daemon": daemon.id.uuidString, "agent": request.agent, "request": request.id]
        post(id: "approval.\(request.id)", content)
    }

    func turnFinished(agent: String, name: String, status: TurnStatus, text: String, daemon: SavedDaemon) {
        let content = UNMutableNotificationContent()
        content.title = switch status {
        case .completed: "\(name) is done"
        case .failed: "\(name) failed"
        case .cancelled: "\(name) was stopped"
        }
        content.subtitle = daemon.name
        content.body = Format.truncate(Format.firstLine(text), 240)
        content.threadIdentifier = agent
        content.userInfo = ["daemon": daemon.id.uuidString, "agent": agent]
        post(id: "turn.\(agent).\(Int(Date().timeIntervalSince1970))", content)
    }

    func removeApproval(_ id: String) {
        center.removeDeliveredNotifications(withIdentifiers: ["approval.\(id)"])
    }

    static func target(of content: UNNotificationContent) -> Target? {
        guard let daemon = (content.userInfo["daemon"] as? String).flatMap(UUID.init(uuidString:)) else { return nil }
        return Target(daemon: daemon, agent: content.userInfo["agent"] as? String, request: content.userInfo["request"] as? String)
    }

    private func post(id: String, _ content: UNNotificationContent) {
        center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil))
    }
}
