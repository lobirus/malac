# Reading an existing product's design language

The goal is to change the product so the change looks like it was always
there. Read the code first: it holds the exact values that a screenshot only
suggests.

## Where to look

Start with `glob` and `grep`, then read what they find.

### Web

- **CSS variables:** `grep -rn -- '--[a-z-]*:' --include=*.css` and the file
  that defines most of them (often `globals.css`, `theme.css`, `tokens.css`,
  `variables.css` or `:root` in the main stylesheet). Check for a dark block:
  `.dark`, `[data-theme="dark"]`, `prefers-color-scheme`, `light-dark()`.
- **Tailwind:** `tailwind.config.{js,ts,cjs,mjs}` (`theme.extend`) for v3;
  `@theme` blocks in CSS for v4. The `cn()` helper and `class-variance-
  authority` variants show how components are composed.
- **shadcn/ui:** `components.json` (style, base colour, CSS variables, icon
  library) and `components/ui/*`. Use those components; add new ones the
  same way.
- **Component libraries:** MUI (`createTheme`, `ThemeProvider`), Chakra
  (`extendTheme`, `createSystem`), Mantine (`createTheme`), Ant Design
  (`ConfigProvider` theme tokens), Radix Themes (`<Theme>` props), Bootstrap
  (`$theme-colors` and `_variables.scss`), Vuetify (`createVuetify` theme),
  PrimeVue/Quasar presets. The theme object is the token file.
- **CSS-in-JS:** styled-components or Emotion `theme.ts`, vanilla-extract
  `createTheme`, Stitches `createStitches`.
- **Preprocessors:** `_variables.scss`, `_tokens.scss`, Less variables.
- **Token pipelines:** `tokens.json`, Style Dictionary configs, Figma token
  exports.
- **Storybook:** `.storybook/` and `*.stories.*` show the intended
  components and states.
- **Brand:** `public/` or `static/` for the logo, favicons and
  `manifest.json` (`theme_color`, `background_color`); fonts imported in the
  entry file or `@font-face` rules.

### Native and other platforms

- **Android Compose:** `ui/theme/Color.kt`, `Theme.kt` (`lightColorScheme`,
  `darkColorScheme`, whether dynamic colour is on), `Type.kt`, `Shape.kt`.
- **Android views:** `res/values/colors.xml`, `themes.xml`, `styles.xml`,
  `dimens.xml`, and the `values-night` variants.
- **iOS and macOS:** `Assets.xcassets/*.colorset/Contents.json` (light and
  dark appearances), `Color`/`Font` extensions, `.tint` and `.accentColor`
  in the app root, custom `ButtonStyle`s and `ViewModifier`s.
- **Flutter:** `ThemeData`, `ColorScheme.fromSeed` or explicit schemes,
  `TextTheme`, `ThemeExtension`s.
- **React Native:** a `theme.ts` or `colors.ts`, `StyleSheet` constants,
  NativeWind's Tailwind config, Tamagui or Restyle themes.
- **Desktop:** Qt stylesheets (`.qss`), GTK CSS, WPF `ResourceDictionary`,
  SwiftUI as above, Electron apps as web.
- **Terminal UIs:** the styles module (`style.rs`, ratatui `Style`
  constants, Bubble Tea `lipgloss` styles, Rich themes, Textual CSS).

### The screens themselves

Read the two or three screens nearest to the one you are changing: the page
layout, header, navigation, how forms, tables, lists, dialogs and toasts are
built, and what empty, loading and error states look like. If the app runs,
take a screenshot of one of them for proportions, but take values from code.

## What to record

Write it down in five to ten lines before changing anything:

- **Colour:** the tokens and what each is used for (primary action,
  accent, surfaces, borders, status). Whether dark mode exists and how it
  is switched.
- **Type:** font families, the size scale, weights, casing (sentence or
  title case), how numbers are set.
- **Shape:** radii, borders or shadows, elevation levels.
- **Space and density:** the spacing scale, card padding, control heights,
  row heights, page widths.
- **Components:** the library and the local wrappers to reuse; the icon set
  and icon sizes.
- **States:** hover, focus, pressed, disabled, loading, empty, error.
- **Motion:** durations, easing, what animates.
- **Copy:** tone, length, how buttons, errors and empty states are worded.
- **Layout patterns:** navigation, page headers, forms, tables, dialogs.

Then build with it. When the product lacks something (no focus ring, no
dark mode, no table style), add the smallest thing that fits its language,
taking the missing values from the house style.
