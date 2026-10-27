import SwiftUI

/// The microphone's recent levels as thin bars, while dictating or talking.
struct LevelMeter: View {
    let levels: [Float]
    var color = Palette.text2
    var height: CGFloat = 24

    var body: some View {
        HStack(alignment: .center, spacing: 2) {
            ForEach(Array(levels.enumerated()), id: \.offset) { _, level in
                RoundedRectangle(cornerRadius: 1)
                    .fill(color)
                    .frame(width: 3, height: 3 + CGFloat(level) * (height - 3))
            }
        }
        .frame(height: height)
        .animation(.linear(duration: 0.08), value: levels)
        .accessibilityHidden(true)
    }
}
