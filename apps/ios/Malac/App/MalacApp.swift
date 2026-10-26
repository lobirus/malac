import SwiftUI
import UserNotifications

@main
struct MalacApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        let model = delegate.model
        WindowGroup {
            RootView()
                .environment(model)
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active: model.enterForeground()
            case .background: model.enterBackground()
            default: break
            }
        }
        .backgroundTask(.appRefresh(AppModel.refreshTask)) {
            await model.checkInBackground()
        }
    }
}

@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    let model = AppModel()

    func application(_ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { await model.register(deviceToken: deviceToken) }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let target = Notifier.target(of: response.notification.request.content) else { return }
        if let action = Notifier.Action(rawValue: response.actionIdentifier) {
            await model.answer(target, action)
        } else if response.actionIdentifier == UNNotificationDefaultActionIdentifier {
            model.open(target)
        }
    }
}
