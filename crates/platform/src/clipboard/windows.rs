//! Windows clipboard capture.
//!
//! A message-only window on its own thread receives `WM_CLIPBOARDUPDATE`
//! (`AddClipboardFormatListener`). It only wakes the reader thread, which
//! reads the newest clipboard state: notifications that arrive while it is
//! reading collapse into one, and the sequence number tells whether anything
//! changed since the last read. Reads retry for up to 5 s while another app
//! holds the clipboard open.

use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use redsnap_core::clipboard::{Capture, Content, Skip};
use windows_sys::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_V,
};
use windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    HWND_MESSAGE, IsWindow, MSG, RegisterClassW, SetForegroundWindow, TranslateMessage, WM_CLIPBOARDUPDATE, WNDCLASSW,
};

use super::convert::{dib_to_png, dropfiles, png_size, png_to_dib, strip_html};
use super::{ClipEvent, ClipEventKind, OnClip, ReadLimits, Target, now_ms};

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;
/// Retry delays for `OpenClipboard`, about 5 s in all. Apps that render
/// formats on request (Excel, remote desktop) can hold it that long.
const RETRY_MS: [u64; 13] = [0, 5, 10, 20, 40, 80, 160, 320, 640, 900, 900, 900, 1000];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn register(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) }
}

/// Registered format ids, looked up once.
struct Formats {
    own: u32,
    html: u32,
    png: u32,
    exclude: u32,
    viewer_ignore: u32,
    history: u32,
}

impl Formats {
    fn get() -> Self {
        Formats {
            own: register("RedSnap.Write"),
            html: register("HTML Format"),
            png: register("PNG"),
            exclude: register("ExcludeClipboardContentFromMonitorProcessing"),
            viewer_ignore: register("Clipboard Viewer Ignore"),
            history: register("CanIncludeInClipboardHistory"),
        }
    }
}

pub struct Watcher {
    /// The message-only window, also the owner for our clipboard writes.
    hwnd: usize,
}

static WAKE: std::sync::OnceLock<SyncSender<()>> = std::sync::OnceLock::new();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        // Coalesce: one pending wake-up is enough, the reader reads the newest state.
        if let Some(tx) = WAKE.get()
            && let Err(TrySendError::Disconnected(_)) = tx.try_send(())
        {
            log::warn!("clipboard reader stopped");
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Watcher {
    pub fn start(limits: ReadLimits, on_clip: OnClip) -> Result<Self> {
        let (wake_tx, wake_rx) = mpsc::sync_channel::<()>(1);
        if WAKE.set(wake_tx).is_err() {
            bail!("the clipboard watcher is already running");
        }
        let (hwnd_tx, hwnd_rx) = mpsc::channel::<Result<usize, String>>();
        thread::Builder::new().name("clipboard-listener".into()).spawn(move || unsafe {
            let class = wide("RedSnapClipboardListener");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: GetModuleHandleW(std::ptr::null()),
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                wc.hInstance,
                std::ptr::null(),
            );
            if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
                let _ = hwnd_tx.send(Err("couldn't listen for clipboard changes".into()));
                return;
            }
            let _ = hwnd_tx.send(Ok(hwnd as usize));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })?;
        let hwnd = hwnd_rx.recv()?.map_err(anyhow::Error::msg)?;
        thread::Builder::new().name("clipboard-reader".into()).spawn(move || {
            let formats = Formats::get();
            let mut last_seq = unsafe { GetClipboardSequenceNumber() };
            for () in wake_rx {
                let seq = unsafe { GetClipboardSequenceNumber() };
                if seq == last_seq {
                    continue;
                }
                let at = now_ms();
                let (source_app, kind) = read(hwnd, &formats, &limits);
                // Reading can render delayed formats, which bumps the number again.
                last_seq = unsafe { GetClipboardSequenceNumber() };
                on_clip(ClipEvent { at, source_app, kind });
            }
        })?;
        Ok(Watcher { hwnd })
    }

    pub fn write(&self, content: &Content, id: &str) -> Result<()> {
        let formats = Formats::get();
        let _open = open(self.hwnd as HWND).ok_or_else(|| anyhow::anyhow!("another app is holding the clipboard"))?;
        unsafe { EmptyClipboard() };
        match content {
            Content::Text { text, html } => {
                let utf16: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                set(CF_UNICODETEXT, bytemuck_u16(&utf16))?;
                if let Some(h) = html {
                    let mut b = h.as_bytes().to_vec();
                    b.push(0);
                    set(formats.html, &b)?;
                }
            }
            Content::Image { png, .. } => {
                set(formats.png, png)?;
                if let Some(dib) = png_to_dib(png) {
                    set(CF_DIB, &dib)?;
                }
            }
            Content::Files(files) => set(CF_HDROP, &dropfiles(files))?,
        }
        set(formats.own, id.as_bytes())?;
        Ok(())
    }
}

fn bytemuck_u16(v: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), v.len() * 2) }
}

/// Copies `bytes` into a movable global block and hands it to the clipboard.
fn set(format: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h.is_null() {
            bail!("out of memory for the clipboard");
        }
        let p = GlobalLock(h);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
        GlobalUnlock(h);
        if SetClipboardData(format, h as HANDLE).is_null() {
            GlobalFree(h);
            bail!("the clipboard refused format {format}");
        }
    }
    Ok(())
}

/// The clipboard, open; closed on drop.
struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

fn open(owner: HWND) -> Option<Open> {
    for ms in RETRY_MS {
        thread::sleep(Duration::from_millis(ms));
        if unsafe { OpenClipboard(owner) } != 0 {
            return Some(Open);
        }
    }
    None
}

/// The bytes of a clipboard format, if present and no larger than `max`.
/// `Err(size)` when it's larger.
fn data(format: u32, max: usize) -> Option<Result<Vec<u8>, u64>> {
    unsafe {
        let h = GetClipboardData(format);
        if h.is_null() {
            return None;
        }
        let size = GlobalSize(h as _);
        if size > max {
            return Some(Err(size as u64));
        }
        let p = GlobalLock(h as _);
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
        GlobalUnlock(h as _);
        Some(Ok(v))
    }
}

fn has(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format) != 0 }
}

/// The executable name of the process that owns the clipboard.
fn owner_app() -> Option<String> {
    unsafe {
        let hwnd = GetClipboardOwner();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        let path = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
        ok.then(|| path.file_name().map(|n| n.to_string_lossy().to_string())).flatten()
    }
}

fn format_names() -> String {
    let mut names = Vec::new();
    let mut f = 0;
    loop {
        f = unsafe { EnumClipboardFormats(f) };
        if f == 0 || names.len() >= 8 {
            break;
        }
        let mut buf = [0u16; 128];
        let n = unsafe { GetClipboardFormatNameW(f, buf.as_mut_ptr(), buf.len() as i32) };
        names.push(if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("#{f}") });
    }
    if names.is_empty() { "empty".into() } else { names.join(", ") }
}

fn read(owner: usize, f: &Formats, limits: &ReadLimits) -> (Option<String>, ClipEventKind) {
    let skip = |s| ClipEventKind::Captured(Capture::Skipped(s));
    let Some(_open) = open(owner as HWND) else {
        return (None, skip(Skip::Unreadable("another app kept the clipboard locked for 5 seconds".into())));
    };
    let app = owner_app();
    if let Some(Ok(id)) = data(f.own, 256) {
        return (app, ClipEventKind::OwnWrite(String::from_utf8_lossy(&id).trim_end_matches('\0').to_string()));
    }
    if has(f.exclude) {
        return (app, skip(Skip::Private("ExcludeClipboardContentFromMonitorProcessing".into())));
    }
    if has(f.viewer_ignore) {
        return (app, skip(Skip::Private("Clipboard Viewer Ignore".into())));
    }
    if let Some(Ok(v)) = data(f.history, 16)
        && v.len() >= 4
        && u32::from_le_bytes([v[0], v[1], v[2], v[3]]) == 0
    {
        return (app, skip(Skip::Private("CanIncludeInClipboardHistory = 0".into())));
    }
    (app, ClipEventKind::Captured(read_content(f, limits)))
}

fn read_content(f: &Formats, limits: &ReadLimits) -> Capture {
    // Files first: Explorer also offers file names as text.
    if has(CF_HDROP) {
        let files = unsafe {
            let h = GetClipboardData(CF_HDROP) as HDROP;
            let n = if h.is_null() { 0 } else { DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0) };
            (0..n.min(10_000))
                .filter_map(|i| {
                    let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0);
                    let mut buf = vec![0u16; len as usize + 1];
                    let got = DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32);
                    (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got as usize])))
                })
                .collect::<Vec<_>>()
        };
        if !files.is_empty() {
            return Capture::Content(Content::Files(files));
        }
    }
    let html = match data(f.html, limits.max_text_bytes) {
        Some(Ok(b)) => Some(String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()),
        _ => None,
    };
    // Text before images: Excel and Word offer a picture of the cells too.
    match data(CF_UNICODETEXT, limits.max_text_bytes * 2) {
        Some(Ok(b)) => {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            if !text.is_empty() {
                return Capture::Content(Content::Text { text, html });
            }
        }
        Some(Err(bytes)) => return Capture::Skipped(Skip::TooLarge { bytes: bytes / 2 }),
        None => {}
    }
    match data(f.png, limits.max_image_bytes) {
        Some(Ok(png)) => {
            if let Some((w, h)) = png_size(&png) {
                return Capture::Content(Content::Image { png, width: w, height: h });
            }
        }
        Some(Err(bytes)) => return Capture::Skipped(Skip::TooLarge { bytes }),
        None => {}
    }
    for fmt in [CF_DIBV5, CF_DIB] {
        match data(fmt, limits.max_image_bytes) {
            Some(Ok(dib)) => {
                if let Some(c) = dib_to_png(&dib) {
                    return Capture::Content(c);
                }
            }
            Some(Err(bytes)) => return Capture::Skipped(Skip::TooLarge { bytes }),
            None => {}
        }
    }
    if let Some(h) = html {
        let text = strip_html(&h);
        if !text.trim().is_empty() {
            return Capture::Content(Content::Text { text, html: Some(h) });
        }
    }
    Capture::Skipped(Skip::Unsupported(format_names()))
}

pub fn foreground() -> Option<Target> {
    let h = unsafe { GetForegroundWindow() };
    (!h.is_null()).then_some(Target(h as usize))
}

pub fn paste_into(target: Target) -> Result<()> {
    let hwnd = target.0 as HWND;
    unsafe {
        if IsWindow(hwnd) == 0 {
            bail!("the app to paste into has closed");
        }
        // RedSnap was in front a moment ago, so it may hand the foreground on.
        SetForegroundWindow(hwnd);
        for _ in 0..50 {
            if GetForegroundWindow() == hwnd {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if GetForegroundWindow() != hwnd {
            bail!("the app to paste into didn't come back to the front; the entry is on the clipboard");
        }
        // Let it settle focus inside its window before the keystroke.
        thread::sleep(Duration::from_millis(30));
        let key = |vk: u16, up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
            },
        };
        let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
        let sent = SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            bail!("Windows blocked the paste keystroke; the entry is on the clipboard");
        }
    }
    Ok(())
}
