---
name: design
description: >
  How to design and build anything people look at: web pages and apps,
  app screens, components, styles, dashboards, charts, emails and terminal
  UIs. Picks the style (the product's own design language, or malac's house
  style for a new product), gives a new product its own identity instead of
  a template, and checks the result in both themes and three widths. Load it
  before writing or changing UI code, every time.
---

# Design

Build interfaces that look designed for this product, by someone who cares,
and that a careful reviewer would not recognise as AI output. Restraint is
the default; character comes from the product, not from decoration.

## 1. Which style applies

Take the first that exists:

1. A style the user or the project's instructions name (AGENTS.md, a
   design doc, a brand guide). Follow it exactly.
2. The product's own design language: its tokens, theme, components and
   screens. See `references/existing-product.md`.
3. Brand assets without a UI yet: a logo, a website, colours in the README.
   Take the colours and type from them, and the rest from the house style.
4. Nothing: the house style, `references/house-style.md`, with its tokens
   in `references/tokens.css`.

An existing product is any project that already has UI in the stack you are
working in. A new screen or component in it is not a new product. A first UI
for a project that had none (a web UI for a CLI tool, a new app in a
monorepo) is a new product, unless the repository has a design system or
brand assets to start from.

## 2. Working in an existing product

1. Study before you change. Read the token or theme file, the shared
   components, and the two or three screens nearest to the one you are
   changing. Prefer code to screenshots: the code tells you the exact values.
2. Write down the system you found in five to ten lines before writing UI
   code: colours and where each is used, type scale, spacing, radii, borders
   versus shadows, density, component library, icon set, copy tone and
   casing, how hover, focus, loading, empty and error states look, motion.
3. Build with what is there. Reuse its components and tokens; add a token to
   its token file instead of a raw value; follow its layout patterns. Match
   copy style, density, states and motion, not only colours.
4. Do not restyle. No new fonts, colour schemes, component libraries or
   "modernising" that nobody asked for. The house style only fills gaps the
   product leaves open, such as a missing focus ring.
5. Where the product is inconsistent, follow its dominant pattern. Mention
   real problems you noticed (contrast, keyboard traps, overflow) instead of
   fixing them outside the task.

## 3. Designing a new product

### The brief

Before code, write a short brief from what the user said and what the
repository shows. Keep it in your notes or your first reply:

- **Who and where:** who uses it, on what (desk, phone, a glance at a wall
  screen), how often.
- **The job:** the one thing they come to do. Design the first screen
  around it.
- **The primary object:** what the screens are about (a run, an invoice, a
  patient, a build). Its name, its states and its key numbers drive the
  layout.
- **Tone:** two adjectives that fit inside the house style (for example
  "calm and exact", "fast and dense").
- **Signature:** one idea that makes this product recognisably itself.

### Identity under restraint

The house style takes colour and decoration off the table on purpose, so
identity has to come from design decisions:

- A layout built around the primary object and the job, not a generic shell.
  A deploy tool might lead with a timeline of runs; a budgeting tool with
  this month's remaining amount.
- Type with intent: a clear scale, weight contrast between the one number
  that matters and everything else, tabular figures that line up.
- Density that fits the use: dense for tools used all day, roomy for
  occasional or public pages.
- The one accent spent on the one thing that matters on each screen.
- The product's own vocabulary in labels, headings and empty states.
- A mark: a simple monochrome logo or wordmark, also used for the favicon.
- One signature element designed with care: a visualisation of the
  product's data, a command bar, an inline editor, a status strip.

Do not use these templates:

- A hero with a gradient, three feature cards with icon tiles, a testimonial
  row and a pricing grid.
- A dashboard opening on four stat tiles with icons and "+12%" badges.
- A sidebar of a dozen icons for a product with three things to do.
- "Welcome back, John", fake avatars, lorem ipsum or invented metrics.
- Cards inside cards, every section boxed, everything centred.
- Sparkles, emoji, glassmorphism or a purple-to-blue anything.

Do not copy another company's distinctive UI, logo or icon set. Take an idea
and make it this product's.

### Questions

The house style answers style questions, so never ask about colours or
fonts. Ask about the product only if the user is there (not on autopilot),
the brief is too vague to design from, and you cannot infer the answer from
the request or the repository. Even then, ask at most three questions, each
with options and your recommendation. Otherwise decide, state your
assumptions in one or two lines, and build.

## 4. Building

1. **Tokens first.** On the web, copy `references/tokens.css` (or extend the
   product's token file). On other platforms, build the theme object from the
   table in `house-style.md`. Components never use raw colours or sizes.
2. **Then components, with every state:** default, hover, focus-visible,
   pressed, disabled, loading and error, and in both themes.
3. **Then screens, with real content:** the actual labels, realistic data
   shapes (long names, zero, many, none) and the empty, loading and error
   states of every list and form.
4. **Keyboard and focus:** the rules in `references/polish.md`.
5. Split large UIs into files of reasonable size (under about 400 lines).

Content rules:

- No filler. Every element earns its place; an empty-feeling section is a
  layout problem, not a reason to invent content. Ask before adding pages or
  sections nobody asked for.
- No invented facts: no made-up metrics, testimonials, customer logos,
  prices or links. When a screen needs sample data to be shown, make it
  plausible, keep it in one fixture file and label it as sample data.
- No fake functionality. A control that does nothing yet is disabled with a
  reason, or left out.
- Copy is short, specific and in sentence case. Buttons say what happens
  ("Save changes", "Delete 3 files"). Errors say what went wrong and what to
  do. No "Step 1:" when the UI numbers the steps already, and no "N/A"
  padding.
- Never rely on colour alone: pair status colour with a word or an icon.
- Show progress: load data progressively, keep the UI usable while it
  loads, and show a skeleton or spinner only where the wait is.
- A placeholder beats a bad drawing. For an illustration, photo or icon you
  do not have, leave a labelled placeholder and say what is needed.

## 5. Verifying

Follow `references/verify.md`. Before you say the UI is done:

- It builds and runs with no console errors.
- You looked at screenshots at 1440, 1024 and 390 px wide, in light and in
  dark, and fixed what was off.
- You walked it with Tab: everything reachable, in order, with a visible
  focus ring, and Esc closes what it opened.
- You went through `references/polish.md` for the screens you touched.

When you cannot run something (no browser, no emulator), say which checks
you could not do.

## 6. Variations

When the user asks to explore, propose or compare designs rather than build
one, make three directions that differ in layout, interaction or density, not
only in colour. Go from by-the-book to bolder, and put them in one place the
user can switch between (a variant switch on one page, or routes). Then say
in a line what each one is trying.

## 7. Reporting

In the final message, give the direction in one line (for a new product:
the brief's job, tone and signature), what you verified, and what you could
not check. Keep it short.
