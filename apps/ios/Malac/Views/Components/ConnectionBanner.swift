import MalacKit
import SwiftUI

/// Says when the selected daemon is not reachable, and why.
struct ConnectionBanner: View {
    @Environment(AppModel.self) private var app
    let session: DaemonSession

    var body: some View {
        if let content {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: content.icon)
                    .foregroundStyle(content.color)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(content.title).font(.footnote.weight(.semibold))
                    if let detail = content.detail {
                        Text(detail).font(.footnote).foregroundStyle(Palette.text2)
                    }
                }
                Spacer(minLength: 0)
                if case .unauthorized = session.status {
                    Button("Pair again") { app.repairing = session.daemon }
                        .font(.footnote.weight(.semibold))
                } else if case .waiting = session.status {
                    Button("Retry") { session.reconnectNow() }
                        .font(.footnote.weight(.semibold))
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Palette.surface2)
            .overlay(alignment: .bottom) { Divider() }
            .accessibilityElement(children: .combine)
        }
    }

    private struct Content {
        var icon: String
        var color: Color
        var title: String
        var detail: String?
    }

    private var content: Content? {
        if let reason = session.state.shutdownReason, session.status != .online {
            return Content(icon: "power", color: Palette.amber, title: "The daemon stopped",
                           detail: "\(reason.prefix(1).uppercased() + reason.dropFirst()). Waiting for it to start again.")
        }
        switch session.status {
        case .online, .stopped:
            return nil
        case .connecting:
            return Content(icon: "arrow.triangle.2.circlepath", color: Palette.text3, title: "Connecting to \(session.daemon.name)…")
        case .waiting(_, let reason):
            return Content(icon: "wifi.exclamationmark", color: Palette.amber, title: "\(session.daemon.name) is not reachable", detail: reason)
        case .unauthorized:
            return Content(icon: "lock.slash", color: Palette.red, title: "This phone is no longer paired",
                           detail: "The device was revoked on the daemon. Pair it again with a new code.")
        }
    }
}
