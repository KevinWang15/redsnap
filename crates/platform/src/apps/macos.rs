//! macOS apps: `.app` bundles in the standard application folders (and one
//! level of subfolders, like `/Applications/Utilities`), named by the
//! Finder's localized display name, identified by their bundle id.

use std::path::{Path, PathBuf};

use anyhow::Result;
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::AppEntry;

const MAX_PER_ROOT: usize = 5_000;

fn roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut r: Vec<PathBuf> = ["/Applications", "/System/Applications", "/System/Library/CoreServices/Applications"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(home) = dirs::home_dir() {
        r.push(home.join("Applications"));
    }
    r.extend(extra.iter().cloned());
    r
}

pub fn scan(extra: &[PathBuf]) -> Result<Vec<AppEntry>> {
    let mut bundles = Vec::new();
    for root in roots(extra) {
        let before = bundles.len();
        collect(&root, 0, &mut bundles, before);
    }
    Ok(bundles.iter().filter_map(|b| entry(b)).collect())
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, start: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if out.len() - start >= MAX_PER_ROOT {
            return;
        }
        let path = e.path();
        if path.extension().is_some_and(|x| x == "app") {
            out.push(path);
        } else if depth < 1 && path.is_dir() {
            // Never inside bundles; one level of plain folders.
            collect(&path, depth + 1, out, start);
        }
    }
}

/// `CFBundleIdentifier`, `CFBundleName`, `CFBundleDisplayName`,
/// `CFBundleExecutable`, `CFBundleIconFile` from `Contents/Info.plist`.
#[derive(Default)]
pub struct BundleInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub executable: Option<String>,
    pub icon_file: Option<String>,
}

pub fn bundle_info(bundle: &Path) -> BundleInfo {
    let Ok(value) = plist::Value::from_file(bundle.join("Contents/Info.plist")) else { return BundleInfo::default() };
    let Some(d) = value.as_dictionary() else { return BundleInfo::default() };
    let s = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_string);
    BundleInfo {
        id: s("CFBundleIdentifier"),
        name: s("CFBundleName"),
        display_name: s("CFBundleDisplayName"),
        executable: s("CFBundleExecutable"),
        icon_file: s("CFBundleIconFile"),
    }
}

#[cfg(target_os = "macos")]
fn finder_name(bundle: &Path) -> Option<String> {
    use objc2_foundation::{NSFileManager, NSString};
    let path = NSString::from_str(&bundle.to_string_lossy());
    let name = NSFileManager::defaultManager().displayNameAtPath(&path).to_string();
    Some(name.strip_suffix(".app").unwrap_or(&name).to_string())
}

#[cfg(not(target_os = "macos"))]
fn finder_name(_: &Path) -> Option<String> {
    None
}

fn entry(bundle: &Path) -> Option<AppEntry> {
    let info = bundle_info(bundle);
    let stem = bundle.file_stem()?.to_string_lossy().to_string();
    let name = finder_name(bundle).or(info.display_name.clone()).unwrap_or_else(|| stem.clone());
    let mut aliases = Vec::new();
    for a in [Some(stem), info.name, info.display_name, info.executable].into_iter().flatten() {
        if a != name && !aliases.contains(&a) {
            aliases.push(a);
        }
    }
    Some(AppEntry {
        key: info.id.unwrap_or_else(|| bundle.display().to_string()).to_lowercase(),
        name,
        aliases,
        subtitle: bundle.display().to_string(),
        launch: LaunchTarget::Path(bundle.to_path_buf()),
        location: Some(bundle.to_path_buf()),
    })
}

/// The bundle's `.icns` icon at the size nearest above `px`, as BGRA. Apps
/// that ship their icon only in an asset catalog get the letter tile until
/// icons come from NSWorkspace.
pub fn icon(target: &LaunchTarget, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let LaunchTarget::Path(bundle) = target else { return None };
    let file = bundle_info(bundle).icon_file.unwrap_or_else(|| "AppIcon".into());
    let file = if file.ends_with(".icns") { file } else { format!("{file}.icns") };
    let family = icns::IconFamily::read(std::io::BufReader::new(std::fs::File::open(bundle.join("Contents/Resources").join(file)).ok()?)).ok()?;
    let mut types = family.available_icons();
    types.sort_by_key(|t| {
        let s = t.pixel_width();
        if s >= px { s - px } else { 10_000 + px - s }
    });
    let image = family.get_icon_with_type(*types.first()?).ok()?;
    let image = image.convert_to(icns::PixelFormat::RGBA);
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut data = image.into_data().into_vec();
    for p in data.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some((w, h, data))
}
