//! macOS snippet insertion and expansion as you type (docs/snippets.md).
//!
//! Insertion reactivates the previous app and types Unicode key events via
//! `CGEventKeyboardSetUnicodeString` (chunked at macOS's 20 UTF-16 unit limit),
//! then moves the caret left for `{{cursor}}`.
//!
//! Expansion as you type installs a listen-only `CGEventTap` on a background
//! `CFRunLoop` thread. The callback only queues events onto a channel so macOS
//! never times out the tap; a worker thread maintains `Typed`, skips RedSnap's
//! own process, excluded apps, secure password inputs (`IsSecureEventInputEnabled`),
//! and active CJK IMEs, and expands matched triggers with synthetic keystrokes
//! tagged with `EVENT_MARKER` so the tap ignores them.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use arc_swap::ArcSwap;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventSource, CGEventSourceStateID,
    CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use redsnap_core::snippets::{self as core, Typed};

use super::{Availability, ExpanderConfig};
use crate::clipboard::Target;
use crate::clipboard::macos::{EVENT_MARKER, accessibility_trusted, activate, current_text};

const VK_RETURN: u16 = 0x24;
const VK_TAB: u16 = 0x30;
const VK_BACKSPACE: u16 = 0x33;
const VK_LEFT: u16 = 0x7B;

/// `CGEventKeyboardSetUnicodeString` accepts at most 20 UTF-16 code units.
const MAX_UNICODE_CHUNK: usize = 20;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFEqual(a: *const c_void, b: *const c_void) -> bool;
    fn CFBooleanGetValue(b: *const c_void) -> bool;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(
        input_source: *const c_void,
        property_key: *const c_void,
    ) -> *const c_void;
    static kTISPropertyInputSourceType: *const c_void;
    static kTISTypeKeyboardInputMode: *const c_void;
    static kTISPropertyInputSourceIsASCIICapable: *const c_void;
}

pub fn availability() -> Availability {
    if accessibility_trusted() {
        Availability::Ready
    } else {
        Availability::NeedsPermission
    }
}

enum Ev {
    Key {
        keycode: u16,
        modifiers_reset: bool,
        ch: Option<char>,
    },
    Click,
}

struct TapContext {
    tx: Sender<Ev>,
    port: Mutex<Option<CFRetained<CFMachPort>>>,
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let ctx = unsafe { &*(user_info as *const TapContext) };
    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            if let Ok(guard) = ctx.port.lock()
                && let Some(port) = guard.as_ref()
            {
                CGEvent::tap_enable(port, true);
            }
            return event.as_ptr();
        }
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            let _ = ctx.tx.send(Ev::Click);
            return event.as_ptr();
        }
        CGEventType::KeyDown => {}
        _ => return event.as_ptr(),
    }

    let ev_ref = unsafe { event.as_ref() };
    if CGEvent::integer_value_field(Some(ev_ref), CGEventField::EventSourceUserData) == EVENT_MARKER
    {
        return event.as_ptr();
    }
    let keycode =
        CGEvent::integer_value_field(Some(ev_ref), CGEventField::KeyboardEventKeycode) as u16;
    let flags = CGEvent::flags(Some(ev_ref));
    let modifiers_reset = flags.intersects(
        CGEventFlags::MaskCommand | CGEventFlags::MaskControl | CGEventFlags::MaskAlternate,
    );
    let ch = if modifiers_reset {
        None
    } else {
        let mut buf = [0u16; 8];
        let mut len = 0usize;
        unsafe {
            CGEvent::keyboard_get_unicode_string(
                Some(ev_ref),
                buf.len() as _,
                &mut len as *mut _ as *mut _,
                buf.as_mut_ptr(),
            );
        }
        decode_utf16_char(&buf[..len.min(buf.len())])
    };
    let _ = ctx.tx.send(Ev::Key {
        keycode,
        modifiers_reset,
        ch,
    });
    event.as_ptr()
}

fn decode_utf16_char(units: &[u16]) -> Option<char> {
    let mut iter = char::decode_utf16(units.iter().copied());
    let first = iter.next()?.ok()?;
    if iter.next().is_some() || first.is_control() {
        return None;
    }
    Some(first)
}

/// `CFRunLoopStop` is documented as thread-safe by CoreFoundation, so sending
/// a retained run loop handle back to the owner thread to stop it is safe.
struct SendRunLoop(CFRetained<CFRunLoop>);
unsafe impl Send for SendRunLoop {}

struct RunningTap {
    run_loop: SendRunLoop,
    tap_thread: Option<thread::JoinHandle<()>>,
    worker_thread: Option<thread::JoinHandle<()>>,
}

impl Drop for RunningTap {
    fn drop(&mut self) {
        self.run_loop.0.stop();
        if let Some(t) = self.tap_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.worker_thread.take() {
            let _ = t.join();
        }
    }
}

pub struct Expander {
    config: Arc<ArcSwap<ExpanderConfig>>,
    tap: Mutex<Option<RunningTap>>,
}

impl Expander {
    pub fn start(config: ExpanderConfig) -> Result<Self> {
        let config = Arc::new(ArcSwap::from_pointee(config));
        let tap = if accessibility_trusted() {
            match start_tap(config.clone()) {
                Ok(t) => Some(t),
                Err(e) => {
                    log::info!("macOS snippet event tap not started: {e:#}");
                    None
                }
            }
        } else {
            log::info!(
                "macOS snippet expansion waiting for Accessibility permission in System Settings"
            );
            None
        };
        Ok(Expander {
            config,
            tap: Mutex::new(tap),
        })
    }

    pub fn update(&self, config: ExpanderConfig) {
        self.config.store(Arc::new(config));
        // If Accessibility permission was granted after startup, attach the
        // event tap without requiring a restart.
        let mut guard = self.tap.lock().unwrap();
        if guard.is_none() && accessibility_trusted() {
            match start_tap(self.config.clone()) {
                Ok(t) => *guard = Some(t),
                Err(e) => log::debug!("macOS snippet event tap retry: {e:#}"),
            }
        }
    }
}

fn start_tap(config: Arc<ArcSwap<ExpanderConfig>>) -> Result<RunningTap> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<SendRunLoop, String>>();

    let tap_thread = thread::Builder::new()
        .name("snippet-tap".into())
        .spawn(move || {
            let mask: CGEventMask = (1 << CGEventType::KeyDown.0)
                | (1 << CGEventType::LeftMouseDown.0)
                | (1 << CGEventType::RightMouseDown.0)
                | (1 << CGEventType::OtherMouseDown.0);
            let ctx = Box::new(TapContext {
                tx,
                port: Mutex::new(None),
            });
            let ctx_ptr = Box::into_raw(ctx);
            let port = unsafe {
                CGEvent::tap_create(
                    CGEventTapLocation::HIDEventTap,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::ListenOnly,
                    mask,
                    Some(tap_callback),
                    ctx_ptr.cast(),
                )
            };
            let Some(port) = port else {
                unsafe { drop(Box::from_raw(ctx_ptr)) };
                let _ = ready_tx.send(Err(
                    "couldn't create CGEventTap (grant Accessibility / Input Monitoring)".into(),
                ));
                return;
            };
            unsafe {
                *(*ctx_ptr).port.lock().unwrap() = Some(port.clone());
            }
            let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
                unsafe { drop(Box::from_raw(ctx_ptr)) };
                let _ = ready_tx.send(Err("couldn't create CFRunLoopSource for event tap".into()));
                return;
            };
            let Some(run_loop) = CFRunLoop::current() else {
                unsafe { drop(Box::from_raw(ctx_ptr)) };
                let _ = ready_tx.send(Err("no CFRunLoop on snippet-tap thread".into()));
                return;
            };
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            CGEvent::tap_enable(&port, true);
            let _ = ready_tx.send(Ok(SendRunLoop(run_loop.clone())));
            CFRunLoop::run();
            CGEvent::tap_enable(&port, false);
            run_loop.remove_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            unsafe { drop(Box::from_raw(ctx_ptr)) };
        })?;

    let run_loop = ready_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("snippet-tap thread exited early"))?
        .map_err(anyhow::Error::msg)?;

    let worker_thread = thread::Builder::new()
        .name("snippet-worker".into())
        .spawn(move || run_worker(rx, config))?;

    Ok(RunningTap {
        run_loop,
        tap_thread: Some(tap_thread),
        worker_thread: Some(worker_thread),
    })
}

fn run_worker(rx: mpsc::Receiver<Ev>, config: Arc<ArcSwap<ExpanderConfig>>) {
    let mut typed = Typed::new();
    let mut last_pid: i32 = -1;
    let self_pid = std::process::id() as i32;

    for ev in rx {
        let (keycode, modifiers_reset, ch) = match ev {
            Ev::Click => {
                typed.reset();
                continue;
            }
            Ev::Key {
                keycode,
                modifiers_reset,
                ch,
            } => (keycode, modifiers_reset, ch),
        };

        let (pid, app_name) = frontmost_app();
        if pid != last_pid {
            typed.reset();
            last_pid = pid;
        }
        if pid == self_pid {
            typed.reset();
            continue;
        }
        let cfg = config.load();
        if !cfg.enabled || cfg.triggers.is_empty() {
            continue;
        }
        if modifiers_reset {
            typed.reset();
            continue;
        }
        if keycode == VK_BACKSPACE {
            typed.backspace();
            continue;
        }
        let Some(c) = ch else {
            typed.reset();
            continue;
        };
        let Some(found) = typed.push(c, &cfg.triggers) else {
            continue;
        };
        let Some(template) = cfg.templates.get(&found.snippet_id) else {
            continue;
        };
        if let Some(why) = blocked(app_name.as_deref(), &cfg.excluded_apps) {
            log::debug!("snippet {} not expanded: {why}", found.snippet_id);
            continue;
        }
        let rendered = core::render(
            template,
            &core::Context {
                now: chrono::Local::now(),
                clipboard: current_text(),
            },
        );
        if let Err(e) = send(found.erase, &rendered.text, rendered.cursor_back) {
            log::warn!("expanding a snippet: {e:#}");
        }
    }
}

fn frontmost_app() -> (i32, Option<String>) {
    autoreleasepool(|_| {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return (-1, None);
        };
        (
            app.processIdentifier(),
            app.localizedName().map(|s| s.to_string()),
        )
    })
}

fn is_ime_input_source_active() -> bool {
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return false;
        }
        let source_type = TISGetInputSourceProperty(source, kTISPropertyInputSourceType);
        let is_input_mode =
            !source_type.is_null() && CFEqual(source_type, kTISTypeKeyboardInputMode);
        let is_ascii = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsASCIICapable);
        let is_ascii_capable = !is_ascii.is_null() && CFBooleanGetValue(is_ascii);
        CFRelease(source);
        is_input_mode && !is_ascii_capable
    }
}

fn blocked(app_name: Option<&str>, excluded: &[String]) -> Option<&'static str> {
    if let Some(app) = app_name {
        let lower = app.to_lowercase();
        if excluded
            .iter()
            .any(|x| lower == *x || lower.trim_end_matches(".app") == x.trim_end_matches(".app"))
        {
            return Some("excluded app");
        }
    }
    if unsafe { IsSecureEventInputEnabled() } {
        return Some("secure input (password field)");
    }
    if is_ime_input_source_active() {
        return Some("an input method is composing");
    }
    None
}

/// Splits UTF-16 code units into chunks of at most `MAX_UNICODE_CHUNK` without
/// cutting a surrogate pair in half.
fn utf16_chunks(units: &[u16]) -> Vec<&[u16]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let mut end = (i + MAX_UNICODE_CHUNK).min(units.len());
        if end < units.len() && (0xD800..=0xDBFF).contains(&units[end - 1]) {
            end -= 1;
        }
        out.push(&units[i..end]);
        i = end;
    }
    out
}

fn post_key(source: Option<&CGEventSource>, vk: u16) -> Result<()> {
    let Some(down) = CGEvent::new_keyboard_event(source, vk, true) else {
        bail!("couldn't create key-down event");
    };
    let Some(up) = CGEvent::new_keyboard_event(source, vk, false) else {
        bail!("couldn't create key-up event");
    };
    CGEvent::set_flags(Some(&down), CGEventFlags::empty());
    CGEvent::set_flags(Some(&up), CGEventFlags::empty());
    CGEvent::set_integer_value_field(Some(&down), CGEventField::EventSourceUserData, EVENT_MARKER);
    CGEvent::set_integer_value_field(Some(&up), CGEventField::EventSourceUserData, EVENT_MARKER);
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
    Ok(())
}

fn post_unicode(source: Option<&CGEventSource>, units: &[u16]) -> Result<()> {
    for chunk in utf16_chunks(units) {
        let Some(down) = CGEvent::new_keyboard_event(source, 0, true) else {
            bail!("couldn't create unicode key-down event");
        };
        let Some(up) = CGEvent::new_keyboard_event(source, 0, false) else {
            bail!("couldn't create unicode key-up event");
        };
        CGEvent::set_flags(Some(&down), CGEventFlags::empty());
        CGEvent::set_flags(Some(&up), CGEventFlags::empty());
        CGEvent::set_integer_value_field(
            Some(&down),
            CGEventField::EventSourceUserData,
            EVENT_MARKER,
        );
        CGEvent::set_integer_value_field(
            Some(&up),
            CGEventField::EventSourceUserData,
            EVENT_MARKER,
        );
        unsafe {
            CGEvent::keyboard_set_unicode_string(Some(&down), chunk.len() as _, chunk.as_ptr());
            CGEvent::keyboard_set_unicode_string(Some(&up), chunk.len() as _, chunk.as_ptr());
        }
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn send(erase: usize, text: &str, cursor_back: usize) -> Result<()> {
    if !accessibility_trusted() {
        bail!("Accessibility permission is needed to type snippets");
    }
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
    let src = source.as_deref();
    for _ in 0..erase {
        post_key(src, VK_BACKSPACE)?;
    }
    if erase > 0 {
        thread::sleep(Duration::from_millis(5));
    }
    let mut buf: Vec<u16> = Vec::new();
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => {
                if !buf.is_empty() {
                    post_unicode(src, &buf)?;
                    buf.clear();
                }
                post_key(src, VK_RETURN)?;
            }
            '\t' => {
                if !buf.is_empty() {
                    post_unicode(src, &buf)?;
                    buf.clear();
                }
                post_key(src, VK_TAB)?;
            }
            c => {
                let mut u = [0u16; 2];
                buf.extend_from_slice(c.encode_utf16(&mut u));
            }
        }
    }
    if !buf.is_empty() {
        post_unicode(src, &buf)?;
    }
    for _ in 0..cursor_back.min(10_000) {
        post_key(src, VK_LEFT)?;
    }
    Ok(())
}

pub fn insert(target: Target, text: &str, cursor_back: usize) -> Result<()> {
    if !accessibility_trusted() {
        bail!("Accessibility permission is needed to insert snippets");
    }
    activate(target)?;
    send(0, text, cursor_back)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_chunking_preserves_surrogate_pairs() {
        // 19 ASCII chars + 1 emoji (2 UTF-16 units) = 21 units.
        // Splitting blindly at 20 would break the surrogate pair.
        let s = format!("{}😀x", "a".repeat(19));
        let units: Vec<u16> = s.encode_utf16().collect();
        let chunks = utf16_chunks(&units);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1].len(), 3);
        let rejoined: String = chunks
            .into_iter()
            .map(|c| String::from_utf16(c).expect("valid utf-16 chunk"))
            .collect();
        assert_eq!(rejoined, s);
    }

    #[test]
    fn decode_utf16_filters_controls_and_multi_char() {
        assert_eq!(decode_utf16_char(&['a' as u16]), Some('a'));
        assert_eq!(decode_utf16_char(&['\r' as u16]), None);
        assert_eq!(decode_utf16_char(&['a' as u16, 'b' as u16]), None);
        let mut emoji = [0u16; 2];
        let slice = '😀'.encode_utf16(&mut emoji);
        assert_eq!(decode_utf16_char(slice), Some('😀'));
    }
}
