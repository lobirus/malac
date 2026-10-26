import MalacKit
import SwiftUI

/// The malac colours from `web/style.css`: neutral black and white with a
/// true near-black dark mode, green for focus, links and status, amber and
/// red for status only.
enum Palette {
    static let page = Color(light: 0xFAFAFA, dark: 0x0A0A0A)
    static let surface = Color(light: 0xFFFFFF, dark: 0x111111)
    static let surface2 = Color(light: 0xF5F5F5, dark: 0x181818)
    static let surface3 = Color(light: 0xEBEBEB, dark: 0x222222)
    static let border = Color(light: 0xE5E5E5, dark: 0x262626)
    static let text = Color(light: 0x0A0A0A, dark: 0xEDEDED)
    static let text2 = Color(light: 0x525252, dark: 0xA3A3A3)
    static let text3 = Color(light: 0x6B6B6B, dark: 0x8A8A8A)
    static let primary = Color(light: 0x0A0A0A, dark: 0xEDEDED)
    static let onPrimary = Color(light: 0xFFFFFF, dark: 0x0A0A0A)
    static let green = Color(light: 0x15803D, dark: 0x4ADE80)
    static let amber = Color(light: 0xA15C07, dark: 0xFBBF24)
    static let red = Color(light: 0xB91C1C, dark: 0xF87171)
    static let dotIdle = Color(light: 0xA3A3A3, dark: 0x5C5C5C)
    static let diffAdded = Color(light: 0xEBF7EF, dark: 0x13261A)
    static let diffRemoved = Color(light: 0xFCECEC, dark: 0x2A1515)

    static func color(_ phase: Phase) -> Color {
        switch phase {
        case .running: green
        case .awaitingApproval: amber
        case .failed: red
        case .done, .stopped: text3
        case .idle: dotIdle
        }
    }
}

extension Color {
    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: UIColor { traits in
            let value = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(
                red: CGFloat((value >> 16) & 0xFF) / 255,
                green: CGFloat((value >> 8) & 0xFF) / 255,
                blue: CGFloat(value & 0xFF) / 255,
                alpha: 1
            )
        })
    }
}

extension View {
    /// A list on the malac page colour instead of the system's grouped grey.
    func malacList() -> some View {
        scrollContentBackground(.hidden).background(Palette.page)
    }

    /// A bordered block: code, outputs, cards. No shadows.
    func card(_ fill: Color = Palette.surface2) -> some View {
        background(fill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Palette.border))
    }
}
