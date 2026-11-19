//! Windows apps: everything in `shell:AppsFolder`, the list Start's "All
//! apps" shows. One enumeration covers Start-menu shortcuts (classic apps)
//! and packaged apps, each with a stable id (a packaged app's AUMID, or a
//! classic app's path, sometimes relative to a known folder) and its
//! localized name.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::AppEntry;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDIBits, GetObjectW, HGDIOBJ,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory, KF_FLAG_DEFAULT,
    SHCreateItemFromParsingName, SHGetKnownFolderItem, SHGetKnownFolderPath, SIGDN, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::core::{GUID, HSTRING, PWSTR};

/// Most entries read; a broken shell extension can't make the scan endless.
const MAX_APPS: usize = 20_000;

fn com() {
    // Per thread; a second call on the same thread is harmless.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

fn take(p: PWSTR) -> String {
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s
}

fn name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe { item.GetDisplayName(kind) }.ok().map(take)
}

/// `{6D809377-...}\Notepad++\notepad++.exe` → `C:\Program Files\Notepad++\notepad++.exe`.
fn resolve_known_folder(id: &str) -> Option<PathBuf> {
    let rest = id.strip_prefix('{')?;
    let (guid, tail) = rest.split_once('}')?;
    let guid = GUID::try_from(guid).ok()?;
    let base = take(unsafe { SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None) }.ok()?);
    Some(PathBuf::from(base).join(tail.trim_start_matches('\\')))
}

/// The file behind a classic app's id, if there is one.
fn file_of(id: &str) -> Option<PathBuf> {
    let path = if id.starts_with('{') { resolve_known_folder(id)? } else { PathBuf::from(id) };
    path.is_absolute().then_some(path)
}

pub fn scan(_extra: &[PathBuf]) -> Result<Vec<AppEntry>> {
    com();
    let folder: IShellItem = unsafe { SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) }
        .context("opening the AppsFolder")?;
    let items: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems) }.context("listing apps")?;
    let mut out = Vec::new();
    loop {
        let mut batch = [None];
        let mut fetched = 0u32;
        if unsafe { items.Next(&mut batch, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(item) = batch[0].take() else { break };
        let (Some(title), Some(id)) = (name(&item, SIGDN_NORMALDISPLAY), name(&item, SIGDN_PARENTRELATIVEPARSING)) else {
            continue;
        };
        // Web links pinned to Start aren't apps.
        if id.starts_with("http://") || id.starts_with("https://") || title.is_empty() {
            continue;
        }
        let file = file_of(&id);
        let packaged = id.contains('!');
        let mut aliases = Vec::new();
        if let Some(stem) = file.as_ref().and_then(|f| f.file_stem()).map(|s| s.to_string_lossy().to_string())
            && !stem.eq_ignore_ascii_case(&title)
        {
            aliases.push(stem);
        }
        let subtitle = match &file {
            Some(f) => f.display().to_string(),
            None if packaged => "App".to_string(),
            None => id.clone(),
        };
        out.push(AppEntry {
            key: id.to_lowercase(),
            name: title,
            aliases,
            subtitle,
            launch: LaunchTarget::AppsFolder(id),
            location: file.filter(|f| f.exists()),
        });
        if out.len() >= MAX_APPS {
            log::warn!("stopped listing apps at {MAX_APPS}");
            break;
        }
    }
    Ok(out)
}

/// The app's icon as straight-alpha BGRA, `px` × `px` (or the nearest size the
/// shell has). Windows hands back premultiplied pixels; those are undone, and
/// a fully transparent result counts as no icon.
pub fn icon(target: &LaunchTarget, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let LaunchTarget::AppsFolder(id) = target else { return None };
    com();
    let item: IShellItem =
        unsafe { SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None) }.ok()?;
    let factory: IShellItemImageFactory = windows::core::Interface::cast(&item).ok()?;
    let size = SIZE { cx: px as i32, cy: px as i32 };
    let bitmap = unsafe { factory.GetImage(size, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK) }.ok()?;
    let result = (|| {
        let mut bm = BITMAP::default();
        let got = unsafe { GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) };
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; w * h * 4];
        let dc = unsafe { CreateCompatibleDC(None) };
        let lines = unsafe { GetDIBits(dc, bitmap, 0, h as u32, Some(px.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS) };
        unsafe {
            let _ = DeleteDC(dc);
        }
        if lines == 0 {
            return None;
        }
        let mut any = false;
        for p in px.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            if a > 0 {
                any = true;
                for c in &mut p[..3] {
                    *c = (*c as u32 * 255 + a / 2).checked_div(a).unwrap_or(0).min(255) as u8;
                }
            }
        }
        any.then_some((w, h, px))
    })();
    unsafe {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
    }
    result
}
