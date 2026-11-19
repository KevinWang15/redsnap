//! The OS calls gpui doesn't offer for a pre-created window: show and hide it,
//! place it, hide it from the compositor until it's painted, paint now.
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

/// Where to put the window: top-left in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
}

#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "windows")]
mod windows {
    use super::{Native, Placement};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CLOAK, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRoundRectRgn, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, SetWindowRgn,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE,
        SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, WM_PAINT,
    };

    fn hwnd(n: Native) -> HWND {
        n.0 as HWND
    }

    /// How the rounded panel's outside is cut off.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Clip {
        /// Windows 11 rounds the window (8 px). The 20 px panel corners leave
        /// small blurred wedges outside them.
        Dwm,
        /// A window region the shape of the panel: exact, but aliased.
        Region,
        None,
    }

    pub fn setup(n: Native, clip: Clip, panel_radius: f32) {
        let h = hwnd(n);
        unsafe {
            let dark: i32 = 1;
            DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, (&dark as *const i32).cast(), 4);
            let corner = if clip == Clip::Dwm { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
            DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&corner as *const i32).cast(), 4);
            if clip == Clip::Region {
                let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetWindowRect(h, &mut r);
                let d = (panel_radius * scale(n) * 2.0).round() as i32;
                let rgn = CreateRoundRectRgn(0, 0, r.right - r.left + 1, r.bottom - r.top + 1, d, d);
                SetWindowRgn(h, rgn, 0);
            }
        }
    }

    pub fn scale(n: Native) -> f32 {
        unsafe { GetDpiForWindow(hwnd(n)) as f32 / 96.0 }
    }

    /// Centered horizontally on the monitor under the cursor, a fifth of the
    /// way down its work area. `size` is in logical pixels.
    pub fn placement_at_cursor(n: Native, size: (f32, f32)) -> Placement {
        unsafe {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(mon, &mut info);
            let w = info.rcWork;
            // The window's DPI is the one it was last on; good enough for a
            // spike (the real app re-places on WM_DPICHANGED).
            let s = scale(n);
            let (pw, ph) = ((size.0 * s) as i32, (size.1 * s) as i32);
            Placement { x: w.left + (w.right - w.left - pw) / 2, y: w.top + (w.bottom - w.top - ph) / 5 }
        }
    }

    pub fn place(n: Native, at: Placement) {
        unsafe { SetWindowPos(hwnd(n), HWND_TOPMOST, at.x, at.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) };
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

    use super::{Native, Placement};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Clip {
        Dwm,
        Region,
        None,
    }

    fn conn() -> Option<&'static (RustConnection, usize)> {
        static CONN: OnceLock<Option<(RustConnection, usize)>> = OnceLock::new();
        CONN.get_or_init(|| x11rb::connect(None).map_err(|e| log::warn!("x11 connect: {e}")).ok()).as_ref()
    }

    pub fn setup(_: Native, _: Clip, _: f32) {}

    pub fn placement_at_cursor(_: Native, size: (f32, f32)) -> Placement {
        let Some((c, screen)) = conn() else { return Placement { x: 0, y: 0 } };
        let s = &c.setup().roots[*screen];
        let (w, h) = (s.width_in_pixels as i32, s.height_in_pixels as i32);
        Placement { x: (w - size.0 as i32) / 2, y: (h - size.1 as i32) / 5 }
    }

    pub fn place(n: Native, at: Placement) {
        if let Some((c, _)) = conn() {
            let aux = ConfigureWindowAux::new().x(at.x).y(at.y).stack_mode(StackMode::ABOVE);
            let _ = c.configure_window(n.0 as u32, &aux);
            let _ = c.flush();
        }
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
    use super::{Native, Placement};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Clip {
        Dwm,
        Region,
        None,
    }

    pub fn setup(_: Native, _: Clip, _: f32) {}
    pub fn placement_at_cursor(_: Native, _: (f32, f32)) -> Placement {
        Placement { x: 0, y: 0 }
    }
    pub fn place(_: Native, _: Placement) {}
    pub fn set_cloaked(_: Native, _: bool) {}
    pub fn show(_: Native) {}
    pub fn hide(_: Native) {}
    pub fn paint_now(_: Native) {}
    pub fn take_focus(_: Native) {}
    pub const CAN_PAINT_NOW: bool = false;
}
