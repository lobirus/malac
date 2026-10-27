---
name: design-review
description: >
  Review a UI's design and polish: whether it follows the product's own
  design language (or malac's house style for a product without one), the
  polish checklist, keyboard and focus behaviour, both themes and three
  widths. Reports findings with file:line, and fixes them when asked. Use for
  "review the design", "does this look right", "polish the UI", "design QA",
  /design-review.
argument-hint: "[page, URL, component or files]"
---

Review the UI the user names: $ARGUMENTS. If nothing is named, review the UI
changed since the last commit, or the main screens if nothing changed.

Load the `design` skill first; this review applies its references.

## 1. Find the standard

Work out which style applies (section 1 of the `design` skill). For an
existing product, read its design language as `references/existing-product.md`
in the design skill describes, and write it down in a few lines. Judge the UI
against that, not against your own taste or the house style.

## 2. Look at it

Run it and follow `references/verify.md` from the design skill: screenshots
at 1440, 1024 and 390 px in light and dark, the audit script, a keyboard
walk, and the empty, loading and error states. Then read the code of the
screens and components in scope.

## 3. Check

- **Consistency:** raw colours or sizes where tokens exist, components
  rebuilt instead of reused, spacing, radii or control heights off the
  scale, copy that does not match the product's tone or casing.
- **Polish:** every item in `references/polish.md`.
- **Accessibility:** contrast, names and labels, keyboard operation, focus
  order and visibility, colour as the only signal, touch target sizes.
- **Content:** filler, invented data, controls that do nothing, vague
  errors, missing empty states.
- **Identity**, for a product using the house style: whether the first
  screen serves the product's main job, and which parts read as a template.

## 4. Report

Findings first, most harmful to users first. For each: what is wrong, where
(`path:line`, and the width and theme where it shows), why it matters in one
line, and the fix. Group small repeated issues into one finding. Then list
what you checked and what you could not.

If the user asked for fixes ("review and fix", "polish the UI"), fix the
findings, verify again with the same screenshots, and report what changed.
Otherwise ask whether to fix them, and which.
