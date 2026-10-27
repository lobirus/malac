# House style

malac's default look for new products. It applies when the project has no
design language of its own and the user's instructions name no other style.
An existing product's own style always wins; see `existing-product.md`.

The idea: clean, modern, neutral and monochrome-first, with the restraint of
Vercel or Linear. Not generic blue SaaS, and not retro, editorial or warm.
Character comes from the product itself (see "Identity" in `SKILL.md`), not
from decoration.

## Colour

Tokens for both themes are in `tokens.css`. Copy them; do not retype values.

| Role | Light | Dark |
|---|---|---|
| Page | `#FAFAFA` | `#0A0A0A` |
| Card, popover | `#FFFFFF` | `#111111` |
| Subtle fill (hover, inputs, code) | `#F4F4F5` | `#18181B` |
| Border / strong border | `#E4E4E7` / `#D4D4D8` | `#27272A` / `#3F3F46` |
| Text | `#0A0A0A` | `#FAFAFA` |
| Secondary / tertiary text | `#52525B` / `#71717A` | `#A1A1AA` / `#8B8B94` |
| Primary button, active nav ("ink") | `#0A0A0A` on `#FAFAFA` text | `#FAFAFA` on `#0A0A0A` text |
| Accent: links, focus ring, chart highlight | `#15803D` (`#16A34A` for non-text such as chart lines) | `#22C55E` |
| Success / warning / danger | `#15803D` / `#B45309` / `#DC2626` | `#22C55E` / `#F59E0B` / `#F87171` |

- Primary actions and the active navigation item are solid ink, not accent.
- There is one accent, green, and it is used only for links, focus rings and
  the highlighted series of a chart. Never as a fill for large areas.
- Status colours are for status only: a badge, an inline message, a dot.
- Set `color-scheme` so native controls, scrollbars and form fields follow
  the theme. Follow the system theme by default; offer a light/dark/system
  switch when the product has settings.
- Tertiary text (`#71717A`) passes AA on page and card backgrounds but not on
  the subtle fill; use secondary text there.

## Type

- One modern sans: Geist (`@fontsource-variable/geist`, family
  `"Geist Variable"`) or Inter Tight (`@fontsource-variable/inter-tight`),
  self-hosted. No web-font CDN, no serif, no second display face.
- Numbers, costs, IDs, dates and times use the same sans with
  `font-variant-numeric: tabular-nums`.
- Monospace (Geist Mono, `@fontsource-variable/geist-mono`) only inside real
  code blocks, snippets and terminal output. Not for labels, IDs or headings.
- A dense app scale: 12, 13, 14 (body), 16, 20, 24, 32 px. Weights 400, 500
  and 600; 600 for headings, 500 for buttons and labels.
- Sentence case everywhere: titles, buttons, menu items, table headers.
- `text-wrap: balance` on headings and `text-wrap: pretty` on paragraphs.

## Shape and space

- Radii 4 px (controls inside controls, badges) and 6 px (buttons, inputs,
  cards, menus, dialogs). A full radius only for avatars and status dots.
- 1 px hairline borders separate things. No drop shadows; a menu, popover or
  dialog gets a border and the card colour, and dialogs a dimmed backdrop.
- One spacing scale: 4, 8, 12, 16, 24, 32, 48 px. Cards use one padding
  (16 px, or 24 px for roomy pages) across the product.
- One control height scale: 28 (compact, in tables and toolbars), 32
  (default) and 38 px (prominent forms). Touch layouts use at least 44 px hit
  areas.
- Icons are 14 or 16 px line icons from one set (Lucide is a good default),
  `stroke-width` 1.5 to 2, centred with their label (`inline-flex`,
  `align-items: center`, `gap: 6px`).
- Dense tables: 32 to 36 px rows, numeric columns right-aligned with tabular
  figures, the header aligned with its cells, long values cut with an
  ellipsis and their full text in a `title`.

## Motion

Restrained: 120 to 200 ms, ease-out, for state changes only (hover, open,
close, a row appearing). No bouncing, parallax, looping or scroll-triggered
animation. Respect `prefers-reduced-motion`.

## Charts

Neutral greys for context series, the accent for the one series that matters,
thin (1.5 px) lines, no area fills, no 3D, light gridlines or none, direct
labels where they fit. Tooltips use the card colour and a hairline border.

## Never

- Blue, indigo or violet primaries; brown, brass, amber or paper tones as a
  theme.
- Gradients, glassmorphism and backdrop blur, glows, drop shadows.
- Coloured icon tiles, sparkle icons, emoji as decoration.
- Pills for everything: badges and buttons use the 4 to 6 px radii.
- A rounded card with a coloured left border as an accent.
- Illustrations drawn ad hoc in SVG. Use a clearly marked placeholder and ask
  for the real asset.

## Assets

- Tokens live in one place (CSS variables, or the platform's theme object)
  for both themes. Components never use raw colour values.
- Favicon as SVG, a 32 px PNG and a 180 px `apple-touch-icon`, in the same
  style: an ink mark, optionally with the accent. Set `theme-color` for both
  themes:
  `<meta name="theme-color" content="#FAFAFA" media="(prefers-color-scheme: light)">`
  and the same with `#0A0A0A` for dark.

## Other platforms

The same palette, radii, hairlines and restraint, in the platform's terms.

- **Android (Compose):** a `MaterialTheme` with a light and a dark
  `ColorScheme` built from the table above (dynamic colour off), primary =
  ink, `tertiary` = accent; shapes 4/6 dp; no tonal elevation or shadows;
  edge-to-edge with insets applied.
- **iOS (SwiftUI):** the system font, colour assets with light and dark
  appearances from the table, `.tint` set to ink, hairline separators,
  standard controls before custom ones.
- **Terminal UIs:** the terminal's default foreground and background, greys
  for secondary text, green for focus and success, yellow and red for status,
  no emoji, box-drawing borders only where they separate.
