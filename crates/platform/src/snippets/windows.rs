//! Windows snippet expansion.
//!
//! A low-level keyboard hook and a mouse hook run on their own thread. Their
//! callbacks only queue an event (Windows drops hooks that take too long);
//! a worker thread decodes keys with the foreground app's keyboard layout,
//! tracks what was typed, and expands a matched trigger by sending
//! backspaces and the text in one `SendInput` batch. RedSnap's own
//! keystrokes carry a marker and are ignored by the hook.
//!
//! What was typed resets on a click, a focus change, navigation keys and
//! Ctrl/Alt/Win chords, so a stale prefix never expands. Nothing expands in
//! excluded apps, password fields, or while a Chinese/Japanese IME is in its
//! native input mode (letters are composing there, not typed).

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;

use anyhow::{Result, bail};
use arc_swap::ArcSwap;
use redsnap_core::snippets::{self as core, Typed};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::Ime::ImmGetDefaultIMEWnd;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, ToUnicodeEx, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_LEFT, VK_LMENU,
    VK_LWIN, VK_MENU, VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GUITHREADINFO, GWL_STYLE, GetClassNameW, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, KBDLLHOOKSTRUCT,
    MSG, SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetWindowsHookExW, TranslateMessage,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use super::ExpanderConfig;
use crate::clipboard::Target;

/// `dwExtraInfo` on RedSnap's own keystrokes ("RSNP").
const MARKER: usize = 0x5253_4E50;
/// Edit control style: a password field.
const ES_PASSWORD: isize = 0x20;
const WM_IME_CONTROL: u32 = 0x0283;
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IME_CMODE_NATIVE: usize = 0x0001;

enum Ev {
    Key {
        vk: u32,
        scan: u32,
        down: bool,
    },
    /// A click: whatever was typed may no longer be where the caret is.
    Click,
}

static EVENTS: OnceLock<Sender<Ev>> = OnceLock::new();

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if k.dwExtraInfo != MARKER
            && let Some(tx) = EVENTS.get()
        {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam as u32, WM_KEYUP | WM_SYSKEYUP);
            if down || up {
                let _ = tx.send(Ev::Key {
                    vk: k.vkCode,
                    scan: k.scanCode,
                    down,
                });
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
        )
        && let Some(tx) = EVENTS.get()
    {
        let _ = tx.send(Ev::Click);
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

pub struct Expander {
    config: Arc<ArcSwap<ExpanderConfig>>,
}

impl Expander {
    pub fn start(config: ExpanderConfig) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        if EVENTS.set(tx).is_err() {
            bail!("snippet expansion is already running");
        }
        let config = Arc::new(ArcSwap::from_pointee(config));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        thread::Builder::new()
            .name("snippet-hooks".into())
            .spawn(move || unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
                let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
                if kb.is_null() || mouse.is_null() {
                    let _ = ready_tx.send(Err(
                        "couldn't watch the keyboard (SetWindowsHookEx failed)".into(),
                    ));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                // Low-level hooks run on this thread's message loop.
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })?;
        ready_rx.recv()?.map_err(anyhow::Error::msg)?;
        let shared = config.clone();
        thread::Builder::new()
            .name("snippet-worker".into())
            .spawn(move || run(rx, shared))?;
        Ok(Expander { config })
    }

    pub fn update(&self, config: ExpanderConfig) {
        self.config.store(Arc::new(config));
    }
}

#[derive(Default)]
struct Mods {
    shift: bool,
    ctrl: bool,
    alt: bool,
    win: bool,
}

fn run(rx: mpsc::Receiver<Ev>, config: Arc<ArcSwap<ExpanderConfig>>) {
    let mut typed = Typed::new();
    let mut mods = Mods::default();
    let mut last_window: usize = 0;
    for ev in rx {
        let (vk, scan, down) = match ev {
            Ev::Click => {
                typed.reset();
                continue;
            }
            Ev::Key { vk, scan, down } => (vk, scan, down),
        };
        match vk as u16 {
            VK_SHIFT | 0xA0 | 0xA1 => {
                mods.shift = down;
                continue;
            }
            VK_CONTROL | 0xA2 | VK_RCONTROL => {
                mods.ctrl = down;
                continue;
            }
            VK_MENU | VK_LMENU | VK_RMENU => {
                mods.alt = down;
                continue;
            }
            VK_LWIN | VK_RWIN => {
                mods.win = down;
                continue;
            }
            _ => {}
        }
        if !down {
            continue;
        }
        let window = unsafe { GetForegroundWindow() } as usize;
        if window != last_window {
            typed.reset();
            last_window = window;
        }
        let cfg = config.load();
        if !cfg.enabled || cfg.triggers.is_empty() {
            continue;
        }
        if mods.ctrl || mods.alt || mods.win {
            typed.reset();
            continue;
        }
        if vk as u16 == VK_BACK {
            typed.backspace();
            continue;
        }
        let Some(c) = decode(vk, scan, mods.shift, window as HWND) else {
            // Navigation, function keys, Enter, Tab, Esc, dead keys.
            typed.reset();
            continue;
        };
        let Some(found) = typed.push(c, &cfg.triggers) else {
            continue;
        };
        let Some(template) = cfg.templates.get(&found.snippet_id) else {
            continue;
        };
        if let Some(why) = blocked(window as HWND, &cfg.excluded_apps) {
            log::debug!("snippet {} not expanded: {why}", found.snippet_id);
            continue;
        }
        let rendered = core::render(
            template,
            &core::Context {
                now: chrono::Local::now(),
                clipboard: crate::clipboard::windows::current_text(),
            },
        );
        if let Err(e) = send(found.erase, &rendered.text, rendered.cursor_back) {
            log::warn!("expanding a snippet: {e:#}");
        }
    }
}

/// The character a key press types in the foreground app, with that app's
/// keyboard layout. `None` for keys that don't type one character.
fn decode(vk: u32, scan: u32, shift: bool, window: HWND) -> Option<char> {
    unsafe {
        let thread = GetWindowThreadProcessId(window, std::ptr::null_mut());
        let layout = GetKeyboardLayout(thread);
        let mut state = [0u8; 256];
        if shift {
            state[VK_SHIFT as usize] = 0x80;
        }
        if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
            state[VK_CAPITAL as usize] = 0x01;
        }
        let mut buf = [0u16; 8];
        // Flag 4: don't change the keyboard state (keeps dead keys working
        // in the app the user is typing in).
        let n = ToUnicodeEx(
            vk,
            scan,
            state.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            4,
            layout,
        );
        if n != 1 {
            return None;
        }
        char::from_u32(buf[0] as u32).filter(|c| !c.is_control())
    }
}

/// Why nothing should expand here, if anything.
fn blocked(window: HWND, excluded: &[String]) -> Option<&'static str> {
    unsafe {
        if let Some(app) = crate::clipboard::windows::exe_name_of_window(window) {
            let app = app.to_lowercase();
            if excluded
                .iter()
                .any(|x| app == *x || app.trim_end_matches(".exe") == x.trim_end_matches(".exe"))
            {
                return Some("excluded app");
            }
        }
        // A password field (standard Edit controls; others can't be told apart).
        let thread = GetWindowThreadProcessId(window, std::ptr::null_mut());
        let mut gui: GUITHREADINFO = std::mem::zeroed();
        gui.cbSize = size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(thread, &mut gui) != 0 && !gui.hwndFocus.is_null() {
            let mut class = [0u16; 32];
            let n = GetClassNameW(gui.hwndFocus, class.as_mut_ptr(), class.len() as i32);
            let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
            if class.eq_ignore_ascii_case("Edit")
                && GetWindowLongPtrW(gui.hwndFocus, GWL_STYLE) & ES_PASSWORD != 0
            {
                return Some("password field");
            }
        }
        // An IME in native mode: letters are being composed, not typed.
        let ime = ImmGetDefaultIMEWnd(if gui.hwndFocus.is_null() {
            window
        } else {
            gui.hwndFocus
        });
        if !ime.is_null() {
            let query = |what: usize| {
                let mut result: usize = 0;
                let ok = SendMessageTimeoutW(
                    ime,
                    WM_IME_CONTROL,
                    what,
                    0,
                    SMTO_ABORTIFHUNG,
                    50,
                    &mut result,
                );
                (ok != 0).then_some(result)
            };
            if query(IMC_GETOPENSTATUS).is_some_and(|open| open != 0)
                && query(IMC_GETCONVERSIONMODE).is_some_and(|mode| mode & IME_CMODE_NATIVE != 0)
            {
                return Some("an input method is composing");
            }
        }
    }
    None
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: MARKER,
            },
        },
    }
}

fn unicode(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: unit,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: MARKER,
            },
        },
    }
}

/// Erases `erase` characters, types `text` (newlines as Enter, tabs as Tab),
/// then moves the caret `cursor_back` characters left: one `SendInput`.
fn send(erase: usize, text: &str, cursor_back: usize) -> Result<()> {
    let mut inputs = Vec::with_capacity((erase + text.len() + cursor_back) * 2);
    for _ in 0..erase {
        inputs.push(key(VK_BACK, false));
        inputs.push(key(VK_BACK, true));
    }
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => {
                inputs.push(key(VK_RETURN, false));
                inputs.push(key(VK_RETURN, true));
            }
            '\t' => {
                inputs.push(key(VK_TAB, false));
                inputs.push(key(VK_TAB, true));
            }
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    inputs.push(unicode(*u, false));
                    inputs.push(unicode(*u, true));
                }
            }
        }
    }
    for _ in 0..cursor_back.min(10_000) {
        inputs.push(key(VK_LEFT, false));
        inputs.push(key(VK_LEFT, true));
    }
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent as usize != inputs.len() {
        bail!("Windows accepted {sent} of {} keystrokes", inputs.len());
    }
    Ok(())
}

pub fn insert(target: Target, text: &str, cursor_back: usize) -> Result<()> {
    crate::clipboard::windows::activate(target)?;
    send(0, text, cursor_back)
}
