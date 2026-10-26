import MalacKit
import SwiftUI

/// The web UI's status dot: green and pulsing while running, amber when an
/// approval is waiting, red on failure, a ring when stopped.
struct StatusDot: View {
    let phase: Phase
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        Group {
            if phase == .stopped {
                Circle().strokeBorder(Palette.text3, lineWidth: 1.5)
            } else {
                Circle().fill(Palette.color(phase))
            }
        }
        .frame(width: 8, height: 8)
        .opacity(dim ? 0.35 : 1)
        .onAppear { pulse(phase) }
        .onChange(of: phase) { _, new in pulse(new) }
        .accessibilityLabel(phase.label)
    }

    private func pulse(_ phase: Phase) {
        guard phase == .running, !reduceMotion else {
            dim = false
            return
        }
        withAnimation(.easeInOut(duration: 0.8).repeatForever()) { dim = true }
    }
}

/// A dot with its label, as lists show status.
struct StatusLabel: View {
    let phase: Phase

    var body: some View {
        HStack(spacing: 6) {
            StatusDot(phase: phase)
            Text(phase.label).foregroundStyle(Self.color(phase))
        }
        .font(.footnote)
        .accessibilityElement(children: .combine)
    }

    static func color(_ phase: Phase) -> Color {
        switch phase {
        case .awaitingApproval: Palette.amber
        case .failed: Palette.red
        default: Palette.text2
        }
    }
}
