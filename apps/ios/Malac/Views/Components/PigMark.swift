import SwiftUI

/// The malac pig from `web/favicon.svg`, drawn in its 32 × 32 view box.
struct PigMark: View {
    var body: some View {
        Canvas { context, size in
            let scale = min(size.width, size.height) / 32
            context.scaleBy(x: scale, y: scale)
            let stroke = StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round)
            var outline = Path()
            outline.move(to: CGPoint(x: 7.3, y: 11.9))
            outline.addLine(to: CGPoint(x: 6.4, y: 5.2))
            outline.addLine(to: CGPoint(x: 12.6, y: 9))
            outline.move(to: CGPoint(x: 24.7, y: 11.9))
            outline.addLine(to: CGPoint(x: 25.6, y: 5.2))
            outline.addLine(to: CGPoint(x: 19.4, y: 9))
            outline.addEllipse(in: CGRect(x: 4.5, y: 8.5, width: 23, height: 19))
            outline.addEllipse(in: CGRect(x: 11.6, y: 17.4, width: 8.8, height: 6.4))
            context.stroke(outline, with: .foreground, style: stroke)
            var solid = Path()
            solid.addEllipse(in: CGRect(x: 10.3, y: 14.1, width: 2.6, height: 2.6))
            solid.addEllipse(in: CGRect(x: 19.1, y: 14.1, width: 2.6, height: 2.6))
            solid.addEllipse(in: CGRect(x: 13.7, y: 19.5, width: 1.6, height: 2.2))
            solid.addEllipse(in: CGRect(x: 16.7, y: 19.5, width: 1.6, height: 2.2))
            context.fill(solid, with: .foreground)
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityHidden(true)
    }
}
