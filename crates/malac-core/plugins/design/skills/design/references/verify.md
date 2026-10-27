# Verifying a UI

Look at what you built before you hand it over. Tests and a clean build say
the code runs; only screenshots say it looks right.

## Web, with malac's browser tools

1. Start the dev server with `bash` and `background: true`, and wait until
   its log shows the URL.
2. `browser_open` with the URL. Check for errors: the dev server's log
   (build errors and warnings, failed requests), the page itself (an error
   overlay or a blank screen), and requests that failed:
   `browser_eval` → `performance.getEntriesByType("resource").filter(r => r.responseStatus >= 400).map(r => r.name)`.
   Before you click through the UI, capture runtime errors with
   `browser_eval` → `addEventListener("error", e => (window.__errors ??= []).push(e.message)); addEventListener("unhandledrejection", e => (window.__errors ??= []).push(String(e.reason)))`,
   and read `window.__errors` when you are done. A reload clears both.
3. For each of these, call `browser_emulate`, then `browser_screenshot`
   (with `full_page` for long pages), and look at the image:

   | width × height | color_scheme | mobile |
   |---|---|---|
   | 1440 × 900 | light | false |
   | 1440 × 900 | dark | false |
   | 1024 × 768 | light | false |
   | 390 × 844 | light | true |
   | 390 × 844 | dark | true |

   Look for: overflow and clipped text, misaligned edges, uneven spacing,
   controls of different heights in one row, a select with two chevrons,
   wrapped button labels, unreadable text or invisible borders in dark mode,
   native controls that stayed light in dark mode.
4. At 1440 light and 390 dark, run the audit: read `audit.js` from this
   skill's `references/` and pass its whole text as the `expression` of
   `browser_eval`. Fix every overflow, unnamed control, unlabelled field,
   positive tabindex and contrast failure it reports. `declaredFonts` lists
   the font each text element asks for and `loadedFonts` the web fonts that
   actually loaded: a declared font missing from `loadedFonts` is not
   installed or imported, and the page shows a fallback.
5. Walk the keyboard: `browser_press` `Tab` repeatedly, and after a few
   presses take a screenshot to see the focus ring. `browser_eval` →
   `document.activeElement.outerHTML.slice(0, 200)` tells you where focus
   is. Open each menu and dialog with Enter, move with the arrow keys, close
   with `Escape`, and check focus returns to the control that opened it.
6. Exercise the states: submit an empty form, trigger an error, show an
   empty list, a long name, a slow load. Screenshot the ones you changed.
7. `browser_emulate` with `reset: true` when you are done.

Fix, then look again. Repeat until the screenshots show nothing to fix.

## Web, without the browser tools

When the browser tools are not connected, use Playwright from the command
line (it downloads a browser the first time; ask first if that is not
expected in this project):

```sh
npx -y playwright screenshot --viewport-size=1440,900 --color-scheme=light --full-page http://localhost:5173 /tmp/ui-1440-light.png
npx -y playwright screenshot --viewport-size=390,844 --color-scheme=dark --full-page http://localhost:5173 /tmp/ui-390-dark.png
```

Then `read` the PNG files to look at them. Without a keyboard walk, say that
focus and keyboard behaviour were not checked.

## Android

```sh
adb shell cmd uimode night no    # or yes for dark
adb exec-out screencap -p > /tmp/ui-light.png
```

Check both modes, a small phone and a large one or a tablet if the app
supports it, and the system font scale at 1.3 (`adb shell settings put
system font_scale 1.3`, then back to 1.0). Look for content under the status
bar, camera cutout or navigation bar.

## iOS

```sh
xcrun simctl ui booted appearance dark   # or light
xcrun simctl io booted screenshot /tmp/ui-dark.png
```

Check both appearances, the smallest supported iPhone and Dynamic Type at a
larger size.

## Terminal UIs

Run the program in a fixed-size terminal and capture it, for example with
`tmux new-session -d -x 100 -y 30 'your-command'` and
`tmux capture-pane -p -e`. Check 80 columns and a wide terminal, a light and
a dark terminal theme if colours are involved, and `NO_COLOR=1`.

## Reporting

Say what you checked (widths, themes, keyboard, states) and what you could
not, and why.
