import Foundation

/// The seam for remote push. Local notifications need the app to be running
/// or woken by background refresh, which iOS schedules as it likes; prompt
/// delivery needs APNs, and APNs needs a server holding the app's push key.
/// The daemon cannot be that server (it runs on the user's machine without
/// Apple credentials), so a relay would sit between them:
///
/// 1. The app registers for remote notifications and hands its APNs device
///    token to the daemon it is paired with (a future endpoint, e.g.
///    `POST /api/devices/{id}/push`).
/// 2. When an approval is requested or a turn ends, the daemon posts the
///    token and the ids (no prompts, code or output) to the relay.
/// 3. The relay sends a push; the app fetches the details from the daemon
///    when it opens or when the user acts on the notification.
///
/// Until the relay exists, `NoPush` keeps the app on local notifications.
protocol PushRegistrar {
    /// Called with the APNs device token after `registerForRemoteNotifications`.
    func register(deviceToken: Data, for daemon: SavedDaemon) async
}

struct NoPush: PushRegistrar {
    func register(deviceToken: Data, for daemon: SavedDaemon) async {}
}
