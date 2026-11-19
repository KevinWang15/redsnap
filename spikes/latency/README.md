# M0 latency spike

Throwaway code that answers one question (PLAN.md §0, §7): is a gpui search
window as fast as a hand-written Win32/Direct2D one? Targets: hotkey → painted
window ≤ 16.7 ms p95, keystroke → results frame ≤ 10 ms p95.

What it is:
- One search window, created hidden at startup and never destroyed, drawn to
  docs/design.md with the glass effect.
- 2,000 fixture apps (Latin and CJK names) with generated icons.
- nucleo matching on every keystroke.
- Timing from each trigger to the frame gpui presents, using the
  `PRESENT_HOOK` patch.

## Run

```sh
cargo run --release -p redsnap-spike -- [--paint=stock|now] [--glass=blur|transparent|opaque]
    [--clip=dwm|region|none] [--hotkey=Ctrl+Alt+R] [--bench[=SHOWS]] [--perf=perf.jsonl]
```

- `--paint=stock`: gpui paints on its next vsync tick, as upstream does.
- `--paint=now` (the default): paint right after input. On Windows, a show
  first paints hidden from the compositor (cloaked), then reveals the window.
- `--bench`: drives itself. It shows the window, types queries through gpui's
  own key dispatch, hides it, then writes `<perf>.summary.txt`. On Windows the
  bench window stays cloaked: it is painted but never shown, and it never takes
  focus or input, so nothing appears on the desktop.
- Interactive use: the hotkey toggles the window, Esc hides it, and
  Ctrl/Cmd+Q quits and writes the summary. Every measurement also goes to
  `<perf>.jsonl`, with a log in `<perf>.log`.

Windows build from Linux:

```sh
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build --release -p redsnap-spike --target x86_64-pc-windows-gnu
```

## Results (2026-11-19)

**Windows 11** (Intel RaptorLake iGPU), `--bench=30 --paint=now`, cloaked:

| metric | p50 | p95 | max |
|---|---|---|---|
| hotkey → frame presented | 1.35 ms | 5.76 ms | 6.79 ms |
| keystroke → frame with its results | 0.74 ms | 5.95 ms | 9.82 ms |
| matching 2,000 apps | 0.07 ms | 0.16 ms | 0.53 ms |
| gpui layout + paint | 0.46 ms | 1.06 ms | 2.05 ms |
| gpui layout + paint + present | 0.63 ms | 5.84 ms | 9.75 ms |
| window creation (once, at startup) | 15.5 ms | | |

**Windows 11, by hand** (visible window, real keyboard, Microsoft Pinyin IME),
`--paint=stock`: 3 shows, 68 measured keystrokes. Chinese IME works, and it
felt instant.

| metric | p50 | p95 | max |
|---|---|---|---|
| hotkey → frame presented | 1.58 ms | 1.67 ms | |
| keystroke → frame with its results | 6.29 ms | 9.78 ms | 279.9 ms (one unexplained outlier) |
| gpui layout + paint + present | 0.61 ms | 1.35 ms | 5.63 ms |

The same by hand with `--paint=now`: 3 shows, 31 keystrokes.

| metric | p50 | p95 | max |
|---|---|---|---|
| hotkey → frame presented | 1.52 ms | 1.83 ms | |
| keystroke → frame with its results | 0.82 ms | 3.62 ms | 3.86 ms |
| gpui layout + paint + present | 0.71 ms | 1.13 ms | 1.91 ms |

Both modes felt instant. Stock painting already meets the keystroke target on a visible window: on
average it waits half a refresh. Painting right away (bench above) removes that
wait and leaves more headroom. Before release, find the cause of the 280 ms
outlier (a first-time CJK font fallback load is the first suspect).

**Linux**, Xvfb with software Vulkan, `--bench=20`. Painting right away isn't
wired up on X11 yet, so this is stock painting:

| metric | p50 | p95 |
|---|---|---|
| hotkey → frame presented | 5.0 ms | 8.3 ms |
| keystroke → frame with its results | 13.2 ms | 19.4 ms |
| gpui layout + paint | 0.57 ms | 0.91 ms |
| gpui layout + paint + present | 4.5 ms | 6.2 ms |

Findings:
- **gpui waits for its next vsync tick to paint.** On Windows a background
  thread invalidates every window once per refresh, so stock gpui adds 0–16.7 ms
  to every keystroke. On Linux (X11) a refresh timer does the same; that wait is
  most of the 13 ms above.
- **Painting right away is cheap.** Sending `WM_PAINT` makes gpui draw and present
  within the input handling. The real app needs a proper gpui entry point for
  this (a patch); the spike sends the message directly.
- **Windows sends no `WM_PAINT` to cloaked or off-screen windows.** That rules
  out `RedrawWindow` for painting while cloaked, and it is why the bench can't
  measure stock mode on Windows: stock mode never paints a cloaked window.
- **Not measured yet:** the OS input queue before gpui gets a key, and DWM's
  composition after present. The hands-on run (`run-now.cmd` / `run-stock.cmd`
  on Windows) covers the experience; macOS needs a Mac or a GitHub runner.
