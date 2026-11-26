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
        GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        MonitorFromWindow,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
        SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow,
        USER_DEFAULT_SCREEN_DPI, WM_PAINT,
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
        let width = ((size.0 * scale).round() as i32)
            .min(work.right - work.left)
            .max(1);
        let height = ((size.1 * scale).round() as i32)
            .min(work.bottom - work.top)
            .max(1);
        let left = work.left + (work.right - work.left - width) / 2;
        let top = work.top + (work.bottom - work.top - height) / 5;
        RECT {
            left,
            top,
            right: left + width,
            bottom: top + height,
        }
    }

    fn rects_equal(a: RECT, b: RECT) -> bool {
        a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
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

        // Clamps to small work areas without overflowing.
        let small = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        let clamped = launcher_rect(small, (760., 474.), 144);
        assert_eq!((clamped.right - clamped.left, clamped.bottom - clamped.top), (800, 600));
        assert!(rects_equal(clamped, small));
    }

    /// Queries the destination monitor's effective DPI directly so hidden
    /// windows never size against a stale cached HWND DPI.
    unsafe fn monitor_dpi(mon: HMONITOR, h: HWND) -> u32 {
        let mut dpi_x: u32 = 0;
        let mut dpi_y: u32 = 0;
        if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } == 0
            && dpi_x > 0
        {
            return dpi_x;
        }
        unsafe { GetDpiForWindow(h) }
    }

    unsafe fn target_rect_at_cursor(h: HWND, size: (f32, f32)) -> Option<(HMONITOR, RECT)> {
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) } == 0 {
            return None;
        }
        let mon = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(mon, &mut info) } == 0 {
            return None;
        }
        let dpi = unsafe { monitor_dpi(mon, h) };
        if dpi == 0 {
            return None;
        }
        Some((mon, launcher_rect(info.rcWork, size, dpi)))
    }

    /// Apply the logical size of the borderless popup on every show, using the
    /// destination monitor's effective DPI even when Windows changed display
    /// scale or topology while the window was hidden.
    pub fn place_at_cursor(n: Native, size: (f32, f32)) {
        let h = hwnd(n);
        unsafe {
            let Some((mon, rect)) = target_rect_at_cursor(h, size) else {
                return;
            };
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) != mon {
                // Move onto the destination monitor at the already-scaled target
                // size while cloaked so any synchronous WM_DPICHANGED completes
                // before the final placement call below.
                if SetWindowPos(
                    h,
                    std::ptr::null_mut(),
                    rect.left,
                    rect.top,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                ) == 0
                {
                    return;
                }
            }
            SetWindowPos(
                h,
                HWND_TOPMOST,
                rect.left,
                rect.top,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
    }

    /// Re-applies the scaled placement if showing a previously hidden window
    /// triggered a deferred `WM_DPICHANGED` whose suggested rect shifted or
    /// resized the window away from the target monitor rectangle.
    pub fn ensure_placed_at_cursor(n: Native, size: (f32, f32)) {
        let h = hwnd(n);
        unsafe {
            let Some((_, rect)) = target_rect_at_cursor(h, size) else {
                return;
            };
            let mut current = RECT::default();
            if GetWindowRect(h, &mut current) != 0 && rects_equal(current, rect) {
                return;
            }
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

    /// Process-level setup (App Nap / app icon on macOS; no-op here).
    pub fn init_app() {}

    /// Brings the application to the front (needed for Accessory apps on macOS).
    pub fn activate_app() {}

    /// Calls `on_lock` when the screen locks or the machine sleeps.
    pub fn watch_session_lock(_on_lock: impl Fn() + Send + Sync + 'static) {}

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

    pub fn ensure_placed_at_cursor(_: Native, _: (f32, f32)) {}

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

    pub fn init_app() {}
    pub fn activate_app() {}
    pub fn watch_session_lock(_on_lock: impl Fn() + Send + Sync + 'static) {}

    pub const CAN_PAINT_NOW: bool = false;
}

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
mod macos {
    //! AppKit operations on the pre-created launcher panel: borderless look
    //! with rounded live blur, placement on the screen under the pointer,
    //! alpha-cloaked synchronous first paint, focus, and hiding only the
    //! launcher without hiding the settings window.

    use std::sync::Arc;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{AnyThread, MainThreadMarker, msg_send};
    use objc2_app_kit::{
        NSApplication, NSBezierPath, NSColor, NSEvent, NSImage, NSImageResizingMode, NSScreen,
        NSView, NSVisualEffectView, NSWindow, NSWindowAnimationBehavior,
        NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
        NSWorkspaceWillSleepNotification,
    };
    use objc2_foundation::{
        NSActivityOptions, NSData, NSDistributedNotificationCenter, NSEdgeInsets, NSNotification,
        NSPoint, NSProcessInfo, NSRect, NSSize, NSString,
    };

    use super::{Look, Native};

    /// Native behind-window blur (`NSVisualEffectView` under `GPUIView`) with
    /// 20 px rounded corners masked cleanly on both the backdrop and shadow.
    pub const LOOK: Look = Look {
        blur: true,
        radius: 20.0,
        fill: 0.58,
    };

    const POPUP_WINDOW_LEVEL: isize = 101;

    fn view(n: Native) -> Option<&'static NSView> {
        let ptr = n.0 as *const NSView;
        unsafe { ptr.as_ref() }
    }

    fn window_of(n: Native) -> Option<Retained<NSWindow>> {
        view(n)?.window()
    }

    /// Opts out of App Nap while allowing idle system sleep, so global
    /// hotkeys and clipboard polling stay responsive when RedSnap is hidden.
    pub fn keep_awake() {
        let reason = NSString::from_str("RedSnap global shortcut and clipboard watcher");
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
            &reason,
        );
        std::mem::forget(token);
    }

    /// App-level macOS initialization: disable App Nap and give the process
    /// RedSnap's own icon (used by dialogs, Activity Monitor, and Settings).
    pub fn init_app() {
        keep_awake();
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if let Some(img) = app_icon_nsimage(256) {
            let app = NSApplication::sharedApplication(mtm);
            unsafe { app.setApplicationIconImage(Some(&img)) };
        }
    }

    /// Unhides (if hidden) and brings RedSnap to the front; needed for an
    /// `Accessory` app when opening or focusing the Settings window.
    pub fn activate_app() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if app.isHidden() {
            app.unhideWithoutActivation();
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }

    /// Calls `on_lock` when the macOS screen locks or the system sleeps.
    pub fn watch_session_lock(on_lock: impl Fn() + Send + Sync + 'static) {
        let cb = Arc::new(on_lock);
        unsafe {
            let dist = NSDistributedNotificationCenter::defaultCenter();
            let name = NSString::from_str("com.apple.screenIsLocked");
            let cb1 = cb.clone();
            let block1 = RcBlock::new(move |_notif: std::ptr::NonNull<NSNotification>| {
                cb1();
            });
            let token1 =
                dist.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block1);
            std::mem::forget(token1);

            let ws = NSWorkspace::sharedWorkspace().notificationCenter();
            let block2 = RcBlock::new(move |_notif: std::ptr::NonNull<NSNotification>| {
                cb();
            });
            let token2 = ws.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceWillSleepNotification),
                None,
                None,
                &block2,
            );
            std::mem::forget(token2);
        }
    }

    fn app_icon_nsimage(size: u32) -> Option<Retained<NSImage>> {
        let rgba = crate::icon::app_icon_rgba(size);
        let img = image::RgbaImage::from_raw(size, size, rgba)?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let data = NSData::from_vec(png);
        NSImage::initWithData(NSImage::alloc(), &data)
    }

    /// Stretchable rounded-rectangle mask image so `NSVisualEffectView` clips
    /// its compositor backdrop blur and window shadow to `radius`.
    fn rounded_mask_image(radius: f64) -> Retained<NSImage> {
        let edge = (radius * 2.0 + 1.0).ceil();
        let size = NSSize::new(edge, edge);
        let handler = RcBlock::new(move |rect: NSRect| -> Bool {
            let path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
            NSColor::blackColor().setFill();
            path.fill();
            Bool::YES
        });
        let img = NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler);
        img.setCapInsets(NSEdgeInsets {
            top: radius,
            left: radius,
            bottom: radius,
            right: radius,
        });
        img.setResizingMode(NSImageResizingMode::Stretch);
        img
    }

    /// Configures the launcher panel: borderless, no show/hide animation,
    /// floats above other windows across all Spaces and over fullscreen apps,
    /// with rounded corners on its content view and blur view.
    pub fn setup(n: Native) {
        let Some(ns_view) = view(n) else { return };
        let Some(window) = ns_view.window() else {
            return;
        };
        window.setStyleMask(NSWindowStyleMask::Borderless);
        window.setLevel(POPUP_WINDOW_LEVEL);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setHidesOnDeactivate(false);
        window.setMovable(false);
        window.setHasShadow(true);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));

        let radius = LOOK.radius as f64;
        if let Some(content) = window.contentView() {
            content.setWantsLayer(true);
            if let Some(layer) = content.layer() {
                layer.setCornerRadius(radius);
                layer.setMasksToBounds(true);
            }
            let mask = LOOK.blur.then(|| rounded_mask_image(radius));
            let mut masked = 0usize;
            for sub in content.subviews() {
                if let Ok(vev) = sub.downcast::<NSVisualEffectView>() {
                    if let Some(mask) = &mask {
                        vev.setMaskImage(Some(mask));
                    }
                    vev.setWantsLayer(true);
                    if let Some(layer) = vev.layer() {
                        layer.setCornerRadius(radius);
                        layer.setMasksToBounds(true);
                    }
                    masked += 1;
                }
            }
            log::debug!("macOS launcher setup: masked {masked} NSVisualEffectView(s)");
        }
        // Changing styleMask can clear firstResponder; restore it so typing
        // never triggers the system beep.
        window.makeFirstResponder(Some(ns_view));
    }

    /// Computes the launcher window's AppKit frame (bottom-left origin):
    /// horizontally centered in `visible`, and one fifth of the spare vertical
    /// space down from the top of `visible` (below the menu bar).
    pub(crate) fn launcher_rect(visible: NSRect, size: (f32, f32)) -> NSRect {
        let w = size.0 as f64;
        let h = size.1 as f64;
        let spare_x = (visible.size.width - w).max(0.0);
        let spare_y = (visible.size.height - h).max(0.0);
        let x = (visible.origin.x + spare_x / 2.0).round();
        let top_margin = spare_y / 5.0;
        let y = (visible.origin.y + visible.size.height - top_margin - h)
            .max(visible.origin.y)
            .round();
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    fn point_in_rect(p: NSPoint, r: NSRect) -> bool {
        p.x >= r.origin.x
            && p.x < r.origin.x + r.size.width
            && p.y >= r.origin.y
            && p.y < r.origin.y + r.size.height
    }

    /// Places the launcher on the display containing the mouse cursor,
    /// centered horizontally and a fifth of the way down its visible frame.
    pub fn place_at_cursor(n: Native, size: (f32, f32)) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = window_of(n) else { return };
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let target = screens
            .iter()
            .find(|s| point_in_rect(mouse, s.frame()))
            .or_else(|| NSScreen::mainScreen(mtm))
            .or_else(|| screens.firstObject());
        let Some(screen) = target else { return };
        let rect = launcher_rect(screen.visibleFrame(), size);
        window.setFrame_display(rect, false);
    }

    pub fn ensure_placed_at_cursor(_: Native, _: (f32, f32)) {}

    pub fn set_cloaked(n: Native, cloaked: bool) {
        if let Some(window) = window_of(n) {
            window.setAlphaValue(if cloaked { 0.0 } else { 1.0 });
            if !cloaked {
                window.invalidateShadow();
            }
        }
    }

    pub fn show(n: Native) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = window_of(n) else { return };
        let app = NSApplication::sharedApplication(mtm);
        if app.isHidden() {
            app.unhideWithoutActivation();
        }
        window.orderFrontRegardless();
    }

    /// Hides only the launcher window. If RedSnap is active and no other
    /// RedSnap window (such as Settings) is visible, hides the app so macOS
    /// returns focus immediately to the previously frontmost app.
    pub fn hide(n: Native) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = window_of(n) else { return };
        window.orderOut(None);
        let app = NSApplication::sharedApplication(mtm);
        let self_num = window.windowNumber();
        let other_visible = app
            .windows()
            .iter()
            .any(|w| w.windowNumber() != self_num && w.isVisible() && w.canBecomeKeyWindow());
        if !other_visible && app.isActive() {
            app.hide(None);
        }
    }

    /// Synchronously runs gpui's `CALayerDelegate::displayLayer:` on
    /// `GPUIView`, drawing and presenting the pending frame immediately
    /// instead of waiting for the next vsync tick.
    pub fn paint_now(n: Native) {
        let Some(ns_view) = view(n) else { return };
        let Some(layer) = ns_view.layer() else { return };
        unsafe {
            let view_obj: *const AnyObject = (ns_view as *const NSView).cast();
            let layer_obj: *const AnyObject = Retained::as_ptr(&layer).cast();
            let _: () = msg_send![view_obj, displayLayer: layer_obj];
        }
    }

    pub fn take_focus(n: Native) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(ns_view) = view(n) else { return };
        let Some(window) = ns_view.window() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if app.isHidden() {
            app.unhideWithoutActivation();
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(ns_view));
    }

    pub const CAN_PAINT_NOW: bool = true;

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn positions_launcher_centered_in_visible_frame() {
            // 1470×956 MacBook with 33 pt menu bar and 56 pt Dock at bottom.
            let vis = NSRect::new(NSPoint::new(0.0, 56.0), NSSize::new(1470.0, 867.0));
            let r = launcher_rect(vis, (760.0, 474.0));
            assert_eq!(r.origin.x, 355.0);
            assert_eq!(r.origin.y, 370.0);
            assert_eq!((r.size.width, r.size.height), (760.0, 474.0));

            // Secondary monitor to the left (negative X) with top menu bar.
            let left_mon = NSRect::new(NSPoint::new(-1920.0, 0.0), NSSize::new(1920.0, 1055.0));
            let r = launcher_rect(left_mon, (760.0, 474.0));
            assert_eq!(r.origin.x, -1340.0);
            // top_margin = (1055 - 474) / 5 = 116.2; y = 1055 - 116.2 - 474 = 464.8 -> 465
            assert_eq!(r.origin.y, 465.0);

            // Smaller display than the window: stays pinned to visible origin.
            let tiny = NSRect::new(NSPoint::new(10.0, 20.0), NSSize::new(640.0, 400.0));
            let r = launcher_rect(tiny, (760.0, 474.0));
            assert_eq!(r.origin.x, 10.0);
            assert_eq!(r.origin.y, 20.0);
        }

        #[test]
        fn point_in_rect_checks_half_open_bounds() {
            let r = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1470.0, 956.0));
            assert!(point_in_rect(NSPoint::new(0.0, 0.0), r));
            assert!(point_in_rect(NSPoint::new(735.0, 478.0), r));
            assert!(!point_in_rect(NSPoint::new(1470.0, 478.0), r));
            assert!(!point_in_rect(NSPoint::new(-1.0, 100.0), r));
        }
    }
}
