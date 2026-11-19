# Local changes to gpui 0.2.2

gpui 0.2.2 from crates.io, with `examples/` and `tests/` (and their
`[[example]]`/`[[test]]` entries) removed. All three platforms are kept. Each
change is marked `redsnap patch` in the source.

## Windows

- `src/platform/windows/window.rs`: `WindowKind::PopUp` windows get `WS_POPUP`
  instead of style 0 (`WS_OVERLAPPED`). Windows frames overlapped windows, and
  gpui subtracted that frame from the content height, so a frameless pop-up drew
  short of its window.
- `src/platform/windows/events.rs`: `WM_NCCALCSIZE` skips the client-area insets
  (meant for resizable windows that draw their own title bar) for `WS_POPUP`
  windows.
- `src/platform/windows/events.rs`: on `WM_MOUSELEAVE`, report the cursor's real
  (outside) position as a mouse move. Upstream only clears the hovered flag, and
  hover is hit-tested against the last position seen (still inside), so hover
  styles and the cursor shape could stick.
- `src/platform/windows/window.rs`: a window created hidden (`show: false`) gets
  its bounds at creation (`SetWindowPlacement` with `SW_HIDE`). Upstream stores
  them and applies them on the first `activate()`, so a hidden window the app
  had positioned itself jumped back to its creation bounds the first time it was
  activated. Upstream also deferred maximize/fullscreen requests made while
  hidden; with this change those requests do nothing on a hidden window.
- `src/platform/windows/util.rs`: `CursorStyle::ClosedHand` maps to the move
  cursor (`IDC_SIZEALL`); upstream shows the arrow.

## Linux

- `src/platform/linux/x11/window.rs`: `HasWindowHandle` for X11 windows returns
  the XCB window id. Upstream leaves it `unimplemented!()`, which panics; the app
  needs the id to show, hide and move its pre-created window.

## All platforms

- `src/window.rs`: `PRESENT_HOOK`, called after a window draws and presents a new
  frame with the draw and draw+present durations. Used for latency measurement.

## Build

So a cross build from Linux to `x86_64-pc-windows-gnu` works:

- Release builds don't run fxc.exe. They use the precompiled shader bytes
  committed in `src/platform/windows/shaders_bytes.rs`, whose first line is a
  hash of the `.hlsl` sources; `build.rs` checks it and fails if they differ.
  `GPUI_REGENERATE_SHADERS=1` (with `GPUI_FXC_PATH`) compiles the shaders and
  rewrites that file. The shaders are unchanged from upstream; a regeneration
  script comes with the first `.hlsl` change.
- `build.rs` runs its Windows step for every Windows *target*. Upstream gates it
  on `#[cfg(target_os = "windows")]`, which in a build script is the *host* OS.
- `embed-resource` is an unconditional build dependency (target-specific build
  dependencies follow the host, too).
- `src/taffy.rs`: `0.0_f32`/`1.0_f32` literals, for rustc's
  `float_literal_f32_fallback` future-incompatibility warning.
