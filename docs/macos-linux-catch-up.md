# macOS and Linux catch-up

Notes for an engineer (or AI agent) taking RedSnap on macOS or Linux to the
same state as Windows. Windows is where the app was developed and tested by
hand; on macOS and Linux it compiles and its tests pass in CI, but most
OS-specific paths are stubs or have never run on a real desktop.

Read first: `PLAN.md` (architecture, §5 implementation rules), `docs/design.md`
(the search window's look), `docs/clipboard.md` (clipboard history rules).

## Ground rules

- **Commit, don't push.** Commit to local `main` as you go; the owner pushes.
- **Never touch the user's real data while testing.** Run with
  `--profile=/tmp/some-dir` so config, history and logs go there. Don't write
  the real system clipboard or drive the real desktop with synthetic input
  unless the person at the machine asks for it; use a private display
  (Xvfb) on Linux.
- **Don't regress Windows.** After changes, check it still builds:
  `cargo check -p redsnap --target x86_64-pc-windows-gnu` (needs mingw-w64)
  or let CI build it.
- Keep per-OS code in `crates/platform`; the app crate shouldn't grow
  `cfg(target_os)` branches beyond the few that exist.
- Match the existing style: doc comments that say why, tests next to the code,
  no compatibility shims (nothing is released yet).

## Layout

| Path | What |
|---|---|
| `crates/core` | Search engine, ranking, calculator, config, usage store, clipboard store and search. No OS code; fully tested anywhere. |
| `crates/platform` | Every OS call: `window.rs` (show/hide/place/paint), `apps/` (discovery + icons per OS), `clipboard/` (capture/write/paste), `autostart.rs`, `instance.rs` (single instance), `open.rs` (launch, open, system commands), `tray.rs`. |
| `crates/app` | The `redsnap` binary: gpui search window (`launcher/`), event loop (`app.rs`), shortcuts (`hotkeys.rs`), settings window (`settings/`, gpui components), icons. |
| gpui | The fork at github.com/KevinWang15/gpui, pinned in `Cargo.toml` (PLAN.md §6.1). Changes to gpui go there as commits. |

## Build and run

```sh
cargo test --workspace
cargo build -p redsnap
# A throwaway profile; first start shows the window.
target/debug/redsnap --profile=/tmp/rs1
# From another terminal: toggle / show / settings / quit the running one.
target/debug/redsnap --profile=/tmp/rs1 toggle
# Latency log: shows and keystrokes timed to the presented frame.
target/debug/redsnap --profile=/tmp/rs1 --perf=/tmp/rs1/perf.jsonl
```

Logs: `<profile>/data/logs/redsnap.log` (`RUST_LOG=redsnap=debug` for more).


Linux build dependencies (Debian/Ubuntu): `libxkbcommon-x11-dev libwayland-dev
libvulkan-dev libfontconfig-dev libfreetype-dev libxcb1-dev libx11-xcb-dev
libssl-dev`, plus a Vulkan driver (Mesa; software `lavapipe` works for testing).
macOS: Xcode (gpui compiles Metal shaders; `xcodebuild -downloadComponent
MetalToolchain` if `xcrun metal` is missing).

## Rules that bit on Windows (they apply everywhere)

- **gpui re-entrancy:** OS calls that show, hide, move or activate a window
  can call back into gpui synchronously and panic ("RefCell already
  borrowed") if made inside a gpui update. Make them from a spawned task
  (see `launcher::show`), never from a listener or `update` closure.
- **The search window is created once and never destroyed.** Show/hide is
  done with OS calls on the existing window (`platform::window`). Creating a
  window per show costs 65–115 ms.
- **Paint right after input.** Stock gpui paints only on its next vsync tick
  (up to 16.7 ms later). Windows uses `window::paint_now` after every
  keystroke and on show; macOS/Linux have `CAN_PAINT_NOW = false` today.
- **Hide on blur only after the window has had focus since it was shown.**
  X11 reported a just-mapped (and even a hidden) window active/inactive out
  of order; see `had_focus` in `launcher/mod.rs`.
- **Virtualized lists need a definite height**, or gpui measures every row
  (the clipboard preview took 1.15 s before that fix).

## Current state

| Area | Windows | macOS | Linux |
|---|---|---|---|
| Search window show/hide | done, tested | gpui app-level (`cx.activate`, `cx.hide()`); never run on a Mac | X11 map/unmap via x11rb, tested on Xvfb; **Wayland: never shows** |
| Window placement | monitor under cursor, DPI-aware | not implemented (gpui default position) | screen 0, scale 1.0 |
| Paint right after input | done | no | no |
| Global shortcuts | done | `global-hotkey` (Carbon); untested | `global-hotkey` (X11 XGrabKey); tested on Xvfb; none on Wayland |
| Tray | done | `tray-icon`; untested | `ksni` (StatusNotifierItem); no tray on GNOME without the AppIndicator extension |
| App discovery | AppsFolder, tested | `.app` bundles + Finder names; untested on a real Mac | `.desktop` files, tested |
| App icons | Shell, tested | `.icns` files only (asset-catalog-only apps get a letter tile) | icon theme (hicolor) + pixmaps, tested |
| Launching | done | `open <bundle>`; untested | `gtk-launch` then `gio launch`; untested |
| Clipboard capture | done | **not implemented** | **not implemented** |
| Paste into previous app | done | **not implemented** | **not implemented** |
| Start at login | done | LaunchAgent plist; untested | `~/.config/autostart`; untested |
| Single instance / CLI | done | Unix socket; tested in CI | tested |
| Settings window (gpui) | done | untested (traffic lights over the painted title bar) | tested on Xvfb |
| Snippets: insert from the launcher | done | **not implemented** | **not implemented** |
| Snippets: expand as you type | done (keyboard hook) | **not implemented** | **not implemented** |
| File search (`f …`) | Everything over IPC; unit-tested, needs a live check | Spotlight (`mdfind`); never run on a Mac | plocate/locate; tested with a stand-in `plocate` |
| Folder browsing (typed paths) | done | shared code; untested on a Mac | tested on Xvfb |
| Latency measured | yes (PLAN.md §7) | **no** (CI bench stalled after ~3 shows) | Xvfb only |

## macOS tasks

1. **Run it and fix show/hide/focus.** `launcher::show` calls
   `cx.activate(true)` then `window.activate_window()`; `hide` calls
   `cx.hide()`. Verify: the hotkey (default Option+Space) shows the window
   focused with the caret in the query field; typing works (including a
   Chinese IME); Esc, Enter and clicking elsewhere hide it; focus returns to
   the previous app. The app should have no Dock icon (activation policy
   Accessory; `LSUIElement` in the bundle later). Check the window is
   frameless, floats above other windows, and isn't in Cmd+Tab.
2. **Placement:** centered on the screen with the mouse, about a fifth of the
   way down the visible frame (below the menu bar). Implement in
   `platform/src/window.rs` (`macos` module); gpui's `WindowOptions` /
   display ids may be enough, or use AppKit on the raw `NSView`'s window.
3. **Paint right after input:** find the gpui macOS equivalent of
   `paint_now` (gpui draws from a CVDisplayLink/`displayLayer` callback);
   likely a small change in the gpui fork (a commit there; list it in PLAN.md §6.1).
   Measure with `--perf` before and after.
4. **App Nap:** a hidden launcher is exactly what App Nap throttles. Decide
   and implement (e.g. `NSProcessInfo beginActivityWithOptions:
   NSActivityUserInitiatedAllowingIdleSystemSleep` while running, or
   confirm Carbon hotkey events wake it promptly). Measure hotkey→frame after
   the app has been hidden for a few minutes.
5. **Clipboard capture** (`platform/src/clipboard/`, new `macos.rs`), per
   `docs/clipboard.md`:
   - Poll `NSPasteboard.generalPasteboard.changeCount` every 150 ms on a
     background thread. If it advanced by more than one between polls,
     record `Skip::Replaced { count }`.
   - Read, in order: file URLs (`public.file-url`) → `Content::Files`;
     text (`public.utf8-plain-text`) with HTML (`public.html`) →
     `Content::Text`; images (`public.png`, else `public.tiff` converted to
     PNG) → `Content::Image`.
   - Honor `org.nspasteboard.ConcealedType` and `org.nspasteboard.TransientType`
     (→ `Skip::Private(<type>)`), and size limits (`ReadLimits`).
   - Source app: `NSWorkspace.frontmostApplication` at capture time (the
     pasteboard has no owner); use its localized name.
   - Mark RedSnap's own writes with a private type (e.g.
     `com.kevinwang15.redsnap.write` holding the entry id) and report them
     as `ClipEventKind::OwnWrite`, like Windows does.
   - `Watcher::write` writes the entry back; keep the `Watcher` API as is.
6. **Paste into the previous app:** record the frontmost app at show
   (`clipboard::foreground()` returns a `Target`), activate it after hiding,
   then send Cmd+V with `CGEventCreateKeyboardEvent` + `CGEventPost`. This
   needs the Accessibility permission: detect it
   (`AXIsProcessTrustedWithOptions`), and without it make Enter copy only and
   say so in the footer (the clipboard view's footer, `launcher/view.rs`).
7. **App icons through the system:** replace the `.icns` reader with
   `NSWorkspace.iconForFile:` rendered to a bitmap (straight-alpha BGRA, as
   `apps::icon` returns elsewhere), so apps whose icon is only in
   `Assets.car` get their real icon. Keep it off the UI thread.
8. **Check app discovery and launching:** localized names (set the system to
   Chinese and check 微信, 网易云音乐 etc. appear by name and by pinyin),
   apps in `/Applications/Utilities` and `~/Applications`, launching with
   Enter, "Show in Finder" with Cmd+Enter.
9. **Measure latency** with `--perf` (hotkey → presented frame, keystroke →
    presented frame) on a real Mac, warm and after App Nap, and add the
    numbers to `PLAN.md` §7.
10. **Start at login and the tray:** check the LaunchAgent works across a
    log out / log in, and the menu bar icon's menu (it uses a template image;
    check it reads in light and dark menu bars).

11. **Snippets** (docs/snippets.md): implement `redsnap_platform::snippets`
    for macOS. Inserting: activate the previous app, then type with
    `CGEventKeyboardSetUnicodeString` events (or paste for long text).
    Expansion as you type: a `CGEventTap` on key-down events (needs the
    Accessibility / Input Monitoring permission; without it, say so in
    Settings → Snippets), decoding with the current keyboard layout
    (`UCKeyTranslate`), resetting on clicks and app switches, skipping secure
    input fields (`IsSecureEventInputEnabled`). Reuse
    `redsnap_core::snippets::{Typed, Triggers, render}` as Windows does.

12. **Blur** (docs/design.md): Windows uses gpui's `Blurred` with a fill at
    0.5. On macOS, try `Blurred` too (the native window blur, no permission
    needed): set `LOOK` in `platform/src/window.rs` and check the corners.
13. **File search** (docs/files.md): run `cargo run -p redsnap-platform
    --example find_files -- <words>` and check that `mdfind` answers within a
    few hundred ms. Check that names with accents and CJK characters match,
    and that `.app` bundles open as apps. Also check that `f` in the launcher
    shows the results.

## Linux tasks

1. **Wayland.** Today `platform::window::native()` returns `None` for Wayland
   windows, so `launcher::show` returns early and the window never appears.
   Options: show/hide through gpui's own Wayland window (a gpui patch adding
   `set_visible`), or recreate a layer-shell surface per show if
   that's fast enough. Global shortcuts don't exist on Wayland: document and
   test binding `redsnap toggle` in GNOME/KDE keyboard settings, and consider
   the XDG GlobalShortcuts portal where available. Keep X11 working.
2. **Placement on X11:** the monitor under the pointer (RandR), centered, a
   fifth of the way down its work area; honor the scale factor (today scale
   is fixed at 1.0 and screen 0 is used).
3. **Paint right after input** on X11 (see the macOS item 3; gpui's X11
   client drives frames from a refresh timer).
4. **Clipboard capture** (`platform/src/clipboard/linux.rs`), per
   `docs/clipboard.md`:
   - X11: XFixes `SelectSelectionInput` on `CLIPBOARD` for change
     notifications; read with `ConvertSelection` (targets: `text/uri-list` →
     files, `UTF8_STRING`/`text/plain;charset=utf-8` + `text/html` → text,
     `image/png` → image).
   - Wayland: `ext-data-control-v1` / `zwlr_data_control_manager_v1` where
     the compositor offers it (KDE, wlroots). GNOME doesn't: there the
     history should say capture isn't available on this desktop.
   - Private markers: `x-kde-passwordManagerHint = secret` → `Skip::Private`.
   - Own writes: a private target (`application/x-redsnap-write`).
   - Writing back: own the selection and serve requests (X11 needs a thread
     that answers `SelectionRequest` until another app takes ownership).
5. **Paste into the previous app:** X11: remember the focused window at show
   (`_NET_ACTIVE_WINDOW`), activate it after hiding, send Ctrl+V with XTEST.
   Wayland: generally not possible; Enter copies only, and the footer says so.
6. **Tray on GNOME:** without the AppIndicator extension there's no tray.
   Make sure RedSnap stays usable (it does: hotkey and `redsnap settings`) and
   mention it in the first-run notice.
7. **Check launching** `.desktop` apps on GNOME and KDE (`gtk-launch` may be
   missing; `gio launch` is the fallback; a direct `Exec=` spawn with field
   codes stripped is the last resort), Flatpak and Snap apps, and
   "Show in folder".
8. **Measure latency** on a real X11 desktop with a GPU, and on Wayland once
   it shows.

9. **Snippets** (docs/snippets.md): on X11, inserting via XTEST (activate
    the previous window first) and expansion as you type via the XRecord
    extension (or XInput2 raw key events), decoding with xkbcommon. Wayland
    gives apps no global key access; expansion is unavailable there (say so
    in Settings → Snippets), inserting may work through the portal
    RemoteDesktop/virtual keyboard where offered.

10. **File search** (docs/files.md): install plocate on a real desktop, run
    `sudo updatedb`, and check `f` in the launcher and the `find_files`
    example. Check the "needs plocate" and "index isn't built" rows on a
    machine without it.

## Done when

- The search window, clipboard history (capture, list, preview, paste) and
  the settings window work on a real Mac and a real Linux desktop (X11; Wayland
  as far as the compositor allows), with the gaps that the OS imposes shown
  to the user, not silently missing.
- Latency on each is measured with `--perf` and recorded in `PLAN.md` §7.
- `cargo test --workspace` and clippy pass; Windows
  still builds.
