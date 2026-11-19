//! Windows: Everything (voidtools), asked over its window-message IPC, the
//! protocol its SDK uses (Everything 1.4.1 or later). No SDK DLL: a query is
//! a `WM_COPYDATA` to Everything's hidden window, and the answer comes back
//! as a `WM_COPYDATA` to a message-only window of ours on the files thread.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use redsnap_core::files::{FileHit, Outcome};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, HWND_MESSAGE, MSG,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA, WM_USER, WNDCLASSW,
};

pub const BACKEND: &str = "Everything";
pub const SYNTAX: &str =
    "Type part of a name. Everything's syntax works too: *.pdf, ext:docx, folder:, a\\path\\";

// From the Everything SDK's everything_ipc.h.
/// `WM_USER` commands.
const IPC_IS_DB_LOADED: usize = 401;
/// `COPYDATASTRUCT::dwData` of a query.
const COPYDATA_QUERY2W: usize = 18;
const REQUEST_NAME: u32 = 0x1;
const REQUEST_PATH: u32 = 0x2;
const SORT_DATE_MODIFIED_DESCENDING: u32 = 14;
const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;
/// Everything 1.4's window class, then 1.5 alpha's (its default instance).
const WINDOW_CLASSES: [&str; 2] = [
    "EVERYTHING_TASKBAR_NOTIFICATION",
    "EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)",
];

const SEND_TIMEOUT_MS: u32 = 2000;
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn not_running() -> Outcome {
    Outcome::Unavailable {
        title: "Everything isn't running".into(),
        subtitle: "File search uses Everything: start it, or get it from voidtools.com (Enter)"
            .into(),
        url: Some("https://www.voidtools.com/".into()),
    }
}

fn everything_window() -> Option<HWND> {
    WINDOW_CLASSES.iter().find_map(|class| {
        let class = wide(class);
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        (!hwnd.is_null()).then_some(hwnd)
    })
}

/// `msg` to Everything; `None` when it didn't answer in time.
fn send(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<usize> {
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd,
            msg,
            wparam,
            lparam,
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            &mut result,
        )
    };
    (ok != 0).then_some(result)
}

thread_local! {
    /// The message-only window answers arrive at, made once per thread.
    static REPLY_WINDOW: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    /// The id the current query's answer carries.
    static WAITING_FOR: Cell<u32> = const { Cell::new(0) };
    static NEXT_ID: Cell<u32> = const { Cell::new(1) };
    static REPLY: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn reply_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COPYDATA && lparam != 0 {
        let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        // An answer to an older query that timed out is dropped.
        if cds.dwData == WAITING_FOR.get() as usize && !cds.lpData.is_null() {
            let data =
                unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
            REPLY.set(Some(data.to_vec()));
        }
        return 1;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn reply_window() -> Option<HWND> {
    let existing = REPLY_WINDOW.get();
    if !existing.is_null() {
        return Some(existing);
    }
    let class = wide("RedSnap.EverythingReply");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(reply_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return None;
        }
        REPLY_WINDOW.set(hwnd);
        Some(hwnd)
    }
}

/// Runs this thread's messages until the answer arrives or time is up.
fn wait_for_reply() -> Option<Vec<u8>> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        if let Some(data) = REPLY.take() {
            return Some(data);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        unsafe {
            let ms = (left.as_millis() as u32).min(INFINITE - 1);
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, ms, QS_ALLINPUT);
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
        }
    }
}

/// `EVERYTHING_IPC_QUERY2` followed by the search text.
fn query_message(reply_hwnd: HWND, id: u32, words: &str, limit: usize) -> Vec<u8> {
    let header = [
        reply_hwnd as usize as u32, // window handles fit in 32 bits
        id,
        0, // search flags: any case, names (a word with `\` matches paths)
        0, // offset
        limit as u32,
        REQUEST_NAME | REQUEST_PATH,
        SORT_DATE_MODIFIED_DESCENDING,
    ];
    let mut out: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
    out.extend(wide(words).iter().flat_map(|c| c.to_le_bytes()));
    out
}

pub fn search(words: &str, limit: usize) -> Outcome {
    let Some(everything) = everything_window() else {
        return not_running();
    };
    match send(everything, WM_USER, IPC_IS_DB_LOADED, 0) {
        None => return Outcome::Failed("Everything didn't answer".into()),
        Some(0) => {
            return Outcome::Unavailable {
                title: "Everything is still indexing".into(),
                subtitle: "Results appear once its index has loaded".into(),
                url: None,
            };
        }
        Some(_) => {}
    }
    if words.trim().is_empty() {
        return Outcome::Hits(Vec::new());
    }
    let Some(reply_hwnd) = reply_window() else {
        return Outcome::Failed("can't receive Everything's answers".into());
    };
    let id = NEXT_ID.get();
    NEXT_ID.set(id.wrapping_add(1).max(1));
    WAITING_FOR.set(id);
    REPLY.set(None);
    let mut message = query_message(reply_hwnd, id, words, limit);
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_QUERY2W,
        cbData: message.len() as u32,
        lpData: message.as_mut_ptr().cast(),
    };
    match send(
        everything,
        WM_COPYDATA,
        reply_hwnd as WPARAM,
        &cds as *const COPYDATASTRUCT as LPARAM,
    ) {
        None => return Outcome::Failed("Everything didn't answer".into()),
        Some(0) => {
            return Outcome::Failed(
                "Everything refused the search (RedSnap needs Everything 1.4.1 or later)".into(),
            );
        }
        Some(_) => {}
    }
    match wait_for_reply() {
        Some(data) => match parse(&data, limit) {
            Some(hits) => Outcome::Hits(hits),
            None => Outcome::Failed("Everything's answer couldn't be read".into()),
        },
        None => Outcome::Failed("Everything took too long to answer".into()),
    }
}

/// Reads an `EVERYTHING_IPC_LIST2`: a header, `numitems` × (flags, data
/// offset), and for each item its name then its path, each a character
/// count, the UTF-16 text and a terminator. Offsets are from the start.
fn parse(data: &[u8], limit: usize) -> Option<Vec<FileHit>> {
    let u32_at = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let string_at = |at: usize| -> Option<(String, usize)> {
        let len = u32_at(at)? as usize;
        let bytes = data.get(at + 4..at + 4 + len * 2)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Some((String::from_utf16_lossy(&units), at + 4 + (len + 1) * 2))
    };
    let count = u32_at(4)? as usize;
    if u32_at(12)? & (REQUEST_NAME | REQUEST_PATH) != REQUEST_NAME | REQUEST_PATH {
        return None;
    }
    let mut hits = Vec::with_capacity(count.min(limit));
    for i in 0..count.min(limit) {
        let item = 20 + i * 8;
        let flags = u32_at(item)?;
        let (name, next) = string_at(u32_at(item + 4)? as usize)?;
        let (dir, _) = string_at(next)?;
        // A drive comes as its name (`C:`) and no path.
        let path = if dir.is_empty() {
            PathBuf::from(format!("{name}\\"))
        } else {
            PathBuf::from(dir).join(&name)
        };
        hits.push(FileHit {
            path,
            name,
            is_dir: flags & (ITEM_FOLDER | ITEM_DRIVE) != 0,
            hidden: false,
        });
    }
    Some(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer laid out the way Everything sends it.
    fn answer(items: &[(u32, &str, &str)]) -> Vec<u8> {
        let mut head: Vec<u32> = vec![
            99,
            items.len() as u32,
            0,
            REQUEST_NAME | REQUEST_PATH,
            SORT_DATE_MODIFIED_DESCENDING,
        ];
        let mut strings = Vec::new();
        let data_start = 20 + items.len() * 8;
        let mut offsets = Vec::new();
        for (flags, name, path) in items {
            offsets.push((*flags, (data_start + strings.len()) as u32));
            for s in [name, path] {
                let units: Vec<u16> = s.encode_utf16().collect();
                strings.extend((units.len() as u32).to_le_bytes());
                strings.extend(
                    units
                        .iter()
                        .chain([0u16].iter())
                        .flat_map(|u| u.to_le_bytes()),
                );
            }
        }
        for (f, o) in offsets {
            head.extend([f, o]);
        }
        let mut out: Vec<u8> = head.iter().flat_map(|v| v.to_le_bytes()).collect();
        out.extend(strings);
        out
    }

    #[test]
    fn reads_an_answer() {
        let data = answer(&[
            (0, "report.pdf", "C:\\Users\\me\\Documents"),
            (ITEM_FOLDER, "微信", "D:\\apps"),
            (ITEM_DRIVE, "C:", ""),
        ]);
        let hits = parse(&data, 10).unwrap();
        assert_eq!(
            hits[0].path,
            PathBuf::from("C:\\Users\\me\\Documents\\report.pdf")
        );
        assert_eq!(
            (hits[0].name.as_str(), hits[0].is_dir),
            ("report.pdf", false)
        );
        assert_eq!((hits[1].name.as_str(), hits[1].is_dir), ("微信", true));
        assert_eq!(
            (hits[2].path.clone(), hits[2].is_dir),
            (PathBuf::from("C:\\"), true)
        );
        assert_eq!(parse(&data, 1).unwrap().len(), 1);
        // Cut short: refused, not misread.
        assert!(parse(&data[..data.len() - 6], 10).is_none());
    }

    #[test]
    fn query_layout() {
        let m = query_message(0x1234 as HWND, 7, "ab", 50);
        let words: Vec<u32> = m[..28]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        assert_eq!(words, [0x1234, 7, 0, 0, 50, 3, 14]);
        assert_eq!(&m[28..], &[b'a', 0, b'b', 0, 0, 0]);
    }
}
