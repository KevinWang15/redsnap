# Search window design

The look of the search window, taken from the reference mockup (2026-11-19).
Sizes are logical pixels at a window width of 760. The mockup was measured at
about 1.25× scale, so measurements from it are divided by 1.25.

## Panel

- Matte glass, picked 2026-11-19 from three variants: the desktop under the window,
  blurred about 28 px, shows through a dark fill.
- Fill: a vertical gradient from `rgba(16, 24, 30, 0.80)` at the top to
  `rgba(22, 18, 34, 0.84)` at the bottom (slightly teal → slightly violet).
- The OS blurs are not used. Windows acrylic blurs too strongly to show anything
  and can't be tuned. Instead, RedSnap captures the screen under its window and
  blurs that itself (quarter resolution, three box passes).
- The capture takes 9–13 ms on Windows (GDI waits for composition). By default
  the window waits for it and opens finished, so hotkey to frame is about
  12–17 ms instead of about 2 ms.
- Optional (a setting, not built yet): show at once with
  an opaque fill while excluded from captures (`WDA_EXCLUDEFROMCAPTURE`), capture
  in the background, then fade the fill to 0.8 over 100 ms. It opens faster but
  the fade reads as a background flash, so it is off by default. The exclusion
  is lifted after the capture, so screenshots of RedSnap still work. The window is fully transparent outside the panel, so the panel's
  own 20 px corners are the window's corners.
- Border: 1 px `rgba(255, 255, 255, 0.10)`. Corner radius 20.
- A soft drop shadow outside the panel, if the OS draws one for the window.
- Padding: 16 around the content.
- Backdrop per OS:
  - Windows: the matte glass above (screen capture + our blur), Windows 10 and
    11 alike. Windows' own corner rounding and border are off.
  - macOS: reading the screen needs the Screen Recording permission, which a
    launcher shouldn't ask for. Use the native window blur with its radius
    tuned to match instead.
  - Linux: X11 capture works without a permission; Wayland doesn't allow it, so
    fall back to the fill alone, raised to 0.94.

## Search row

- Query field: height 56, radius 12, fill `rgba(11, 15, 22, 0.90)`, border 1 px
  `rgba(225, 45, 70, 0.45)` (the RedSnap red, dimmed). Text 26 px, white, with a
  1.5 px white caret. Placeholder `rgba(255, 255, 255, 0.35)`.
- Clear button inside the field on the right: a 26 px circle,
  `rgba(255, 255, 255, 0.10)`, with a light "×". Shown only when the query is not
  empty.
- To the right of the field, 12 apart: the RedSnap mark, 44 × 44 with no frame
  or background (changed 2026-11-19): four white corner brackets (13 px, 2.5 px
  stroke, 4 px from the edges) around an 18 px red rounded square, drawn as two
  SVG layers over the same box so they stay centered on each other at any
  display scale.
- Icons are [Lucide](https://lucide.dev) (ISC), compiled into the binary
  (`crates/app/assets/icons`) and tinted with the text color; the settings app
  uses the same set through `lucide-react`.

## Results

- 5 visible rows with a 66 pitch: row height 62, gap 4. The list scrolls past 5.
- Each row has:
  - **Icon**: 38 × 38, 14 from the row's left edge; app icons drawn as is.
  - **Title**: 19 px, white, medium weight.
  - **Subtitle**: 13 px, `rgba(255, 255, 255, 0.55)`. It holds the path, or what
    the item is ("View contact in RedSnap").
  - **Hint**, right-aligned: `↵` on the selected row, and the quick-run shortcut
    on the other rows. That's `⌘2`…`⌘9` on macOS and `Ctrl 2`…`Ctrl 9`
    elsewhere, 15 px, `rgba(255, 255, 255, 0.45)`.
- The selected row:
  - Radius 12.
  - Horizontal gradient `#B3263A` → `#8E1F33`.
  - Border 1 px `rgba(255, 120, 130, 0.45)`.
  - A soft red glow under it: shadow `rgba(200, 30, 50, 0.35)`, blur 18, y 4.
  - Its subtitle becomes `rgba(255, 255, 255, 0.80)`.
- Hovered row (not selected): fill `rgba(255, 255, 255, 0.04)`.

## Footer

- A 1 px divider `rgba(255, 255, 255, 0.07)` above it. Height 44.
- Left side: "5 results" (or "Clipboard · 5 entries"), 13 px,
  `rgba(255, 255, 255, 0.50)`. (The mockup's red dot and "RedSnap" label were
  dropped, 2026-11-19.)
- Right side: key hints, each a 20 px key cap (radius 5, border 1 px
  `rgba(255, 255, 255, 0.18)`) followed by a label 13 px `rgba(255, 255, 255, 0.60)`:
  "↑↓ Navigate", "↵ Open". "⇥ Actions" is added once the action panel exists.

## Window size

- Width 760.
- Height: 16 + 56 (search) + 12 + 5 × 66 (rows) + 44 (footer) + 16 = 474.
- The window keeps this size. With fewer results, the rows below are empty.
- With an empty query, the rows show the most used items.

## Light theme

Phase 1 ships this dark design only. A light variant comes later.
