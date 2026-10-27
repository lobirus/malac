# Polish checklist

Go through it for every screen you built or changed, in both themes. These
are the details that make a UI read as careless or machine-made.

## Controls

- Selects: `appearance: none`, exactly one chevron, vertically centred, with
  right padding so the value never runs under it, and the same height as the
  inputs and buttons next to them.
- One control height scale (28, 32, 38 px in the house style); controls in
  a row share a height. Touch targets are at least 44 px.
- Icons are 14 or 16 px, from one set, centred with their text
  (`inline-flex`, `align-items: center`). Icon-only buttons have an
  `aria-label` and a tooltip.
- Every control has hover, focus-visible, pressed and disabled states, and a
  loading state where it starts work.
- Button and badge labels never wrap (`white-space: nowrap`) and never
  truncate mid-word.

## Layout

- One spacing scale; the same padding in every card; aligned edges.
- No double borders where a bordered thing sits in a bordered thing, and no
  cards inside cards.
- No layout shift when content loads, a scrollbar appears or a state
  changes (reserve space, use fixed control widths where labels change).
- No page-level horizontal scroll at 1440, 1024 or 390 px. Wide tables and
  code scroll inside their own container.
- Long text: names, URLs and IDs cut with an ellipsis and their full value
  in `title`, or wrapped where reading them matters.

## Tables

- Numeric columns right-aligned with tabular figures; the header aligned the
  same way as its cells.
- Consistent row height; long values cut with an ellipsis and a `title`.
- An empty state that says what would appear and how to add it.

## Type and copy

- Sentence case for titles, buttons, menu items and headers.
- Placeholder text meets contrast and is not the only label.
- WCAG AA contrast for all text (4.5:1, or 3:1 for 18 px and up or bold
  14 px and up) and 3:1 for focus rings, input borders and icons that carry
  meaning.
- Numbers, costs, IDs and times use tabular figures. Monospace only in code.
- Copy is specific: no "Lorem", no "Something went wrong" without what to do.

## Theming

- Both themes are checked, including dropdowns, native inputs and date
  pickers, code blocks, toasts, modals, tooltips, charts and scrollbars.
- `color-scheme` is set, so native controls follow the theme.
- Scrollbars are thin and themed.
- `theme-color` meta for both themes, and a favicon in SVG, PNG and
  `apple-touch-icon`.

## Focus and keyboard

- Autofocus the first field on a login page and when a dialog opens, and the
  main input (a composer, a search box) on load and after submitting. Never
  steal focus while the user is doing something else.
- Logical tab order following the visual order; no positive `tabindex`;
  `tabindex="-1"` only for elements focused from code.
- Every custom control works from the keyboard: Enter and Space activate,
  arrow keys move in menus, lists, tabs and radio groups, Esc closes.
- Dialogs trap focus while open and return it to the control that opened
  them.
- A visible focus ring on everything focusable (`:focus-visible`), never
  `outline: none` without a replacement.
- A "Skip to content" link on pages with navigation before the content.

## Tell-tale signs of AI-made UI

Remove these wherever you find them in what you touched:

- Gradients on backgrounds, buttons or text; glows; glassmorphism and
  backdrop blur; drop shadows on everything.
- Blue, indigo or violet as the primary colour by default.
- Emoji in headings, buttons or lists; sparkle icons for anything AI.
- Icons in coloured rounded tiles beside every heading.
- Rounded cards with a coloured left border as an accent.
- Pills for every badge and button.
- Stat tiles with invented numbers and "+12%" badges; charts of made-up
  data; a chart for a single number.
- Generic hero sections and three-card feature grids on a product page that
  needs none.
- Several font families, or a monospace font for labels and IDs.
- Inline SVG illustrations drawn by hand instead of a labelled placeholder.
- Everything centred; everything in a card; equal visual weight on
  everything.
- Filler copy: "Unlock the power of", "Seamlessly", "Elevate your workflow".
