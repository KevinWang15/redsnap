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
        DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_CLOAK, DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRoundRectRgn, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, SetWindowRgn,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE,
        SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
        WDA_NONE, WM_PAINT,
    };

    fn hwnd(n: Native) -> HWND {
        n.0 as HWND
    }

    /// How the window's corners are shaped. Windows 11 draws its own rounded
    /// corner and 1 px border; the panel must match it, or both show.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Clip {
        /// Windows 11 rounds the window (8 px, antialiased, clips the blur);
        /// its border is turned off and the panel uses the same radius.
        Dwm,
        /// No Windows rounding; a window region the shape of the 20 px panel.
        /// Exact, but the edge is aliased.
        Region,
        /// No clipping at all: square window, square blur.
        None,
    }

    /// Windows 11's corner radius for rounded windows, in logical pixels.
    pub const DWM_RADIUS: f32 = 8.0;

    /// `system_backdrop`: Windows 11's own acrylic behind the window
    /// (`DWMWA_SYSTEMBACKDROP_TYPE`), instead of gpui's `Blurred`, which uses
    /// the undocumented `SetWindowCompositionAttribute`.
    pub fn setup(n: Native, clip: Clip, panel_radius: f32, system_backdrop: bool) {
        let h = hwnd(n);
        unsafe {
            if system_backdrop {
                // The backdrop shows through the transparent parts of the
                // frame, so extend the frame over the whole window.
                let all = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
                DwmExtendFrameIntoClientArea(h, &all);
                let acrylic: i32 = DWMSBT_TRANSIENTWINDOW;
                DwmSetWindowAttribute(h, DWMWA_SYSTEMBACKDROP_TYPE as u32, (&acrylic as *const i32).cast(), 4);
            }
            let dark: i32 = 1;
            DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, (&dark as *const i32).cast(), 4);
            let corner = if clip == Clip::Dwm { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
            DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&corner as *const i32).cast(), 4);
            // Only the panel's own border shows.
            let none: u32 = DWMWA_COLOR_NONE;
            DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR as u32, (&none as *const u32).cast(), 4);
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

    /// While excluded, the window doesn't appear in any screen capture (GDI,
    /// DXGI, Windows Graphics Capture), so the matte backdrop can be captured
    /// with the window already on screen. It's switched off again once the
    /// backdrop is read, so screenshots of RedSnap work. Windows 10 2004+;
    /// older versions ignore it. Call from the window's thread.
    pub fn set_capture_excluded(n: Native, excluded: bool) {
        let affinity = if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
        unsafe { SetWindowDisplayAffinity(hwnd(n), affinity) };
    }

    /// The screen pixels under the window, as tightly packed BGRA rows. Call
    /// while the window is hidden or excluded from captures. Any thread.
    pub fn capture_behind(n: Native) -> Option<(usize, usize, Vec<u8>)> {
        use windows_sys::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
            DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
        };
        unsafe {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd(n), &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w <= 0 || h <= 0 {
                return None;
            }
            let screen = GetDC(std::ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let mut out = None;
            if !bmp.is_null() && !bits.is_null() {
                let old = SelectObject(dc, bmp);
                if BitBlt(dc, 0, 0, w, h, screen, r.left, r.top, SRCCOPY) != 0 {
                    let len = (w * h * 4) as usize;
                    out = Some((w as usize, h as usize, std::slice::from_raw_parts(bits as *const u8, len).to_vec()));
                }
                SelectObject(dc, old);
                DeleteObject(bmp);
            }
            DeleteDC(dc);
            ReleaseDC(std::ptr::null_mut(), screen);
            out
        }
    }

    /// The window's size in physical pixels.
    pub fn physical_size(n: Native) -> (usize, usize) {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd(n), &mut r) };
        ((r.right - r.left).max(1) as usize, (r.bottom - r.top).max(1) as usize)
    }
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

    pub fn setup(_: Native, _: Clip, _: f32, _: bool) {}

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

    pub fn capture_behind(_: Native) -> Option<(usize, usize, Vec<u8>)> {
        None
    }

    pub fn set_capture_excluded(_: Native, _: bool) {}

    pub fn physical_size(_: Native) -> (usize, usize) {
        (760, 474)
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
mod macos {
    //! macOS goes through gpui's app-level calls (see launcher.rs); these are
    //! no-ops so the shared code compiles.
    use super::{Native, Placement};

    /// Opts out of App Nap, which throttles a hidden app's timers (the bench
    /// waits between steps would stretch from milliseconds to seconds). The
    /// real app needs a decision here too: a hidden launcher is exactly what
    /// App Nap targets.
    pub fn keep_awake() {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let reason = NSString::from_str("RedSnap latency bench");
        let token = NSProcessInfo::processInfo()
            .beginActivityWithOptions_reason(NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical, &reason);
        std::mem::forget(token);
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Clip {
        Dwm,
        Region,
        None,
    }

    pub fn setup(_: Native, _: Clip, _: f32, _: bool) {}
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

    pub fn capture_behind(_: Native) -> Option<(usize, usize, Vec<u8>)> {
        None
    }

    pub fn set_capture_excluded(_: Native, _: bool) {}

    pub fn physical_size(_: Native) -> (usize, usize) {
        (760, 474)
    }
}
