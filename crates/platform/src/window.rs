//! The OS calls gpui doesn't offer for a pre-created window: show and hide it,
//! place it, hide it from the compositor until it's painted, and paint now.
//!
//! Every call here can make the OS call back into gpui's window procedure
//! synchronously, which panics ("RefCell already borrowed") inside a gpui
//! update. Call them from a spawned task, never from an update or a listener.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// A native window handle we can pass around freely.
#[derive(Clone, Copy, Debug)]
pub struct Native(pub usize);

pub fn native(window: &dyn HasWindowHandle) -> Option<Native> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(Native(h.hwnd.get() as usize)),
        RawWindowHandle::Xlib(h) => Some(Native(h.window as usize)),
        RawWindowHandle::Xcb(h) => Some(Native(h.window.get() as usize)),
        RawWindowHandle::AppKit(h) => Some(Native(h.ns_view.as_ptr() as usize)),
        _ => None,
    }
}

/// How the search window looks on this OS (docs/design.md).
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// The compositor blurs what's behind the window, live (gpui's
    /// `WindowBackgroundAppearance::Blurred`).
    pub blur: bool,
    /// The panel's corner radius, in logical pixels.
    pub radius: f32,
    /// The opacity of the panel's fill.
    pub fill: f32,
}

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "windows")]
mod windows {
    use super::{Look, Native};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CLOAK, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
        SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, USER_DEFAULT_SCREEN_DPI,
        WM_PAINT,
    };

    fn hwnd(n: Native) -> HWND {
        n.0 as HWND
    }

    /// The compositor's acrylic (gpui's `Blurred`) fills the window's
    /// rectangle, so Windows rounds the window itself, at its only radius
    /// (8 px), and the panel uses the same radius. The fill is see-through
    /// enough for the blur; the query field stays nearly opaque.
    pub const LOOK: Look = Look {
        blur: true,
        radius: 8.0,
        fill: 0.5,
    };

    /// Dark mode, Windows' own rounded corners, and no Windows border (the
    /// panel draws its own).
    pub fn setup(n: Native) {
        let h = hwnd(n);
        unsafe {
            let dark: i32 = 1;
            DwmSetWindowAttribute(
                h,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&dark as *const i32).cast(),
                4,
            );
            let corner = DWMWCP_ROUND;
            DwmSetWindowAttribute(
                h,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                (&corner as *const i32).cast(),
                4,
            );
            let none: u32 = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(
                h,
                DWMWA_BORDER_COLOR as u32,
                (&none as *const u32).cast(),
                4,
            );
        }
    }

    fn launcher_rect(work: RECT, size: (f32, f32), dpi: u32) -> RECT {
        let scale = dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32;
        let width = (size.0 * scale).round() as i32;
        let height = (size.1 * scale).round() as i32;
        let left = work.left + (work.right - work.left - width) / 2;
        let top = work.top + (work.bottom - work.top - height) / 5;
        RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    #[test]
    fn positions_the_scaled_popup_in_the_monitor_work_area() {
        let work = RECT {
            left: -1920,
            top: 40,
            right: 0,
            bottom: 1080,
        };
        for (dpi, width, height, left, top) in [
            (96, 760, 474, -1340, 153),
            (120, 950, 593, -1435, 129),
            (144, 1140, 711, -1530, 105),
        ] {
            let r = launcher_rect(work, (760., 474.), dpi);
            assert_eq!((r.left, r.top), (left, top));
            assert_eq!((r.right - r.left, r.bottom - r.top), (width, height));
        }
    }

    /// Apply the logical size of the borderless popup on every show, including
    /// when Windows has changed the display scale while the window was hidden.
    pub fn place_at_cursor(n: Native, size: (f32, f32)) {
        let h = hwnd(n);
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p) == 0 {
                return;
            }
            let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut info) == 0 {
                return;
            }
            let work = info.rcWork;
            if MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) != mon {
                // Move entirely onto the destination while cloaked. GPUI handles
                // WM_DPICHANGED synchronously and may apply Windows' suggested
                // rectangle during this move. Read the new DPI after it returns.
                if SetWindowPos(
                    h,
                    std::ptr::null_mut(),
                    work.left,
                    work.top,
                    (size.0 as i32).min(work.right - work.left),
                    (size.1 as i32).min(work.bottom - work.top),
                    SWP_NOACTIVATE | SWP_NOZORDER,
                ) == 0
                {
                    return;
                }
            }
            let dpi = GetDpiForWindow(h);
            if dpi == 0 {
                return;
            }
            let rect = launcher_rect(work, size, dpi);
            SetWindowPos(
                h,
                HWND_TOPMOST,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE,
            );
        }
    }

    pub fn set_cloaked(n: Native, cloaked: bool) {
        let v: i32 = cloaked as i32;
        unsafe { DwmSetWindowAttribute(hwnd(n), DWMWA_CLOAK as u32, (&v as *const i32).cast(), 4) };
    }

    pub fn show(n: Native) {
        unsafe { ShowWindow(hwnd(n), SW_SHOWNOACTIVATE) };
    }

    pub fn hide(n: Native) {
        unsafe { ShowWindow(hwnd(n), SW_HIDE) };
    }

    /// Paints now instead of on gpui's next vsync tick: gpui's `WM_PAINT`
    /// handler draws and presents if anything changed. Sent directly, because
    /// Windows generates no `WM_PAINT` for a cloaked or off-screen window
    /// (`RedrawWindow(RDW_UPDATENOW)` does nothing there). The real app gets
    /// a proper gpui entry point for this instead.
    pub fn paint_now(n: Native) {
        unsafe { SendMessageW(hwnd(n), WM_PAINT, 0, 0) };
    }

    pub fn take_focus(n: Native) {
        unsafe { SetForegroundWindow(hwnd(n)) };
    }

    pub const CAN_PAINT_NOW: bool = true;
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    //! X11 only: map, unmap and move the window over our own connection.
    //! gpui keeps its own; any client may map or move another's window.

    use std::sync::OnceLock;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt, InputFocus, StackMode};
    use x11rb::rust_connection::RustConnection;

    use super::{Look, Native};

    fn conn() -> Option<&'static (RustConnection, usize)> {
        static CONN: OnceLock<Option<(RustConnection, usize)>> = OnceLock::new();
        CONN.get_or_init(|| {
            x11rb::connect(None)
                .map_err(|e| log::warn!("x11 connect: {e}"))
                .ok()
        })
        .as_ref()
    }

    /// No blur yet: a nearly opaque panel with its own 20 px corners on a
    /// transparent window.
    pub const LOOK: Look = Look {
        blur: false,
        radius: 20.0,
        fill: 0.97,
    };

    pub fn setup(_: Native) {}

    pub fn place_at_cursor(n: Native, size: (f32, f32)) {
        let Some((c, screen)) = conn() else {
            return;
        };
        let s = &c.setup().roots[*screen];
        let (w, h) = (s.width_in_pixels as i32, s.height_in_pixels as i32);
        let aux = ConfigureWindowAux::new()
            .x((w - size.0 as i32) / 2)
            .y((h - size.1 as i32) / 5)
            .stack_mode(StackMode::ABOVE);
        let _ = c.configure_window(n.0 as u32, &aux);
        let _ = c.flush();
    }

    pub fn set_cloaked(_: Native, _: bool) {}

    pub fn show(n: Native) {
        if let Some((c, _)) = conn() {
            let _ = c.map_window(n.0 as u32);
            let _ = c.flush();
        }
    }

    pub fn hide(n: Native) {
        if let Some((c, _)) = conn() {
            let _ = c.unmap_window(n.0 as u32);
            let _ = c.flush();
        }
    }

    pub fn paint_now(_: Native) {}

    pub fn take_focus(n: Native) {
        if let Some((c, _)) = conn() {
            let _ = c.set_input_focus(InputFocus::PARENT, n.0 as u32, x11rb::CURRENT_TIME);
            let _ = c.flush();
        }
    }

    pub const CAN_PAINT_NOW: bool = false;
}

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
mod macos {
    //! macOS goes through gpui's app-level calls (see launcher.rs); these are
    //! no-ops so the shared code compiles.
    use super::{Look, Native};

    /// Opts out of App Nap, which throttles a hidden app's timers (the bench
    /// waits between steps would stretch from milliseconds to seconds). The
    /// real app needs a decision here too: a hidden launcher is exactly what
    /// App Nap targets.
    pub fn keep_awake() {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let reason = NSString::from_str("RedSnap latency bench");
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical,
            &reason,
        );
        std::mem::forget(token);
    }

    /// No blur yet: a nearly opaque panel with its own 20 px corners on a
    /// transparent window.
    pub const LOOK: Look = Look {
        blur: false,
        radius: 20.0,
        fill: 0.97,
    };

    pub fn setup(_: Native) {}
    pub fn place_at_cursor(_: Native, _: (f32, f32)) {}
    pub fn set_cloaked(_: Native, _: bool) {}
    pub fn show(_: Native) {}
    pub fn hide(_: Native) {}
    pub fn paint_now(_: Native) {}
    pub fn take_focus(_: Native) {}
    pub const CAN_PAINT_NOW: bool = false;
}
