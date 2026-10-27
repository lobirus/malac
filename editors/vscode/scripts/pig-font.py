"""Builds media/malac-icons.woff: the malac pig as an icon-font glyph (U+E000),
so the status bar can show it as $(malac-pig). Same drawing as media/malac.svg
(a 32-unit grid), with strokes turned into filled outlines, in the metrics of
VS Code's codicon font (300 units per em, sitting on the baseline).

    python3 scripts/pig-font.py     # needs fontTools
"""

import math
import pathlib

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

UPEM = 300
SCALE = UPEM / 32
HALF = 1.25  # half the stroke width, in SVG units
OUT = pathlib.Path(__file__).resolve().parent.parent / "media" / "malac-icons.woff"


def point(x, y):
    """SVG (y down) to font units (y up)."""
    return (round(x * SCALE), round((32 - y) * SCALE))


def area(points):
    return sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(points, points[1:] + points[:1])) / 2


def oriented(points, clockwise=True):
    # In font space a negative signed area is clockwise: an outer contour.
    return points if (area(points) < 0) == clockwise else points[::-1]


def ellipse(cx, cy, rx, ry, steps=56):
    return [point(cx + rx * math.cos(2 * math.pi * i / steps), cy + ry * math.sin(2 * math.pi * i / steps)) for i in range(steps)]


def ring(cx, cy, rx, ry):
    return [oriented(ellipse(cx, cy, rx + HALF, ry + HALF)), oriented(ellipse(cx, cy, rx - HALF, ry - HALF), clockwise=False)]


def polyline(points):
    """A stroked polyline: one quad per segment and a disc at each point (round joins and caps)."""
    contours = []
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        length = math.hypot(x1 - x0, y1 - y0)
        nx, ny = -(y1 - y0) / length * HALF, (x1 - x0) / length * HALF
        quad = [point(x0 + nx, y0 + ny), point(x1 + nx, y1 + ny), point(x1 - nx, y1 - ny), point(x0 - nx, y0 - ny)]
        contours.append(oriented(quad))
    for x, y in points:
        contours.append(oriented(ellipse(x, y, HALF, HALF, steps=16)))
    return contours


def pig():
    contours = []
    contours += ring(16, 18, 11.5, 9.5)  # head
    contours += ring(16, 20.6, 4.4, 3.2)  # snout
    contours += polyline([(7.3, 11.9), (6.4, 5.2), (12.6, 9.0)])  # ears
    contours += polyline([(24.7, 11.9), (25.6, 5.2), (19.4, 9.0)])
    for cx, cy, rx, ry in [(11.6, 15.4, 1.45, 1.45), (20.4, 15.4, 1.45, 1.45), (14.5, 20.6, 0.85, 1.15), (17.5, 20.6, 0.85, 1.15)]:
        contours.append(oriented(ellipse(cx, cy, rx, ry, steps=20)))  # eyes and nostrils
    pen = TTGlyphPen(None)
    for contour in contours:
        pen.moveTo(contour[0])
        for p in contour[1:]:
            pen.lineTo(p)
        pen.closePath()
    return pen.glyph()


def main():
    builder = FontBuilder(UPEM, isTTF=True)
    builder.setupGlyphOrder([".notdef", "pig"])
    builder.setupCharacterMap({0xE000: "pig"})
    builder.setupGlyf({".notdef": TTGlyphPen(None).glyph(), "pig": pig()})
    glyph = builder.font["glyf"]["pig"]
    glyph.recalcBounds(builder.font["glyf"])
    builder.setupHorizontalMetrics({".notdef": (UPEM, 0), "pig": (UPEM, glyph.xMin)})
    builder.setupHorizontalHeader(ascent=UPEM, descent=0)
    builder.setupNameTable({"familyName": "malac-icons", "styleName": "Regular"})
    builder.setupOS2(sTypoAscender=UPEM, sTypoDescender=0, usWinAscent=UPEM, usWinDescent=0)
    builder.setupPost()
    builder.font.flavor = "woff"
    builder.save(str(OUT))
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
