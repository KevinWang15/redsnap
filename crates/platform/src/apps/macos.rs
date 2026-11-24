//! macOS apps: `.app` bundles in the standard application folders (and one
//! level of subfolders, like `/Applications/Utilities`), named by the
//! Finder's localized display name, identified by their bundle id.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::AppEntry;

const MAX_PER_ROOT: usize = 5_000;

fn roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut r: Vec<PathBuf> = [
        "/Applications",
        "/System/Applications",
        "/System/Library/CoreServices/Applications",
    ]
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
    // Finder lives directly in CoreServices rather than CoreServices/Applications.
    let finder = PathBuf::from("/System/Library/CoreServices/Finder.app");
    if finder.is_dir() && !bundles.contains(&finder) {
        bundles.push(finder);
    }
    let mut entries: Vec<AppEntry> = bundles.iter().filter_map(|b| entry(b)).collect();
    disambiguate_keys(&mut entries);
    Ok(entries)
}

/// Keeps side-by-side versions of an app that share a `CFBundleIdentifier`
/// (e.g. Adobe Illustrator 2026 and 2027) from colliding in the catalog while
/// leaving unique bundle ids unchanged.
fn disambiguate_keys(entries: &mut [AppEntry]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in entries.iter() {
        *counts.entry(e.key.clone()).or_default() += 1;
    }
    for e in entries.iter_mut() {
        if counts.get(&e.key).copied().unwrap_or(0) > 1 {
            let path = e
                .location
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| e.subtitle.clone())
                .to_lowercase();
            e.key = format!("{}::{path}", e.key);
        }
    }
}

fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, start: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
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
    let Ok(value) = plist::Value::from_file(bundle.join("Contents/Info.plist")) else {
        return BundleInfo::default();
    };
    let Some(d) = value.as_dictionary() else {
        return BundleInfo::default();
    };
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
    let name = NSFileManager::defaultManager()
        .displayNameAtPath(&path)
        .to_string();
    Some(name.strip_suffix(".app").unwrap_or(&name).to_string())
}

#[cfg(not(target_os = "macos"))]
fn finder_name(_: &Path) -> Option<String> {
    None
}

fn entry(bundle: &Path) -> Option<AppEntry> {
    let info = bundle_info(bundle);
    let stem = bundle.file_stem()?.to_string_lossy().to_string();
    let name = finder_name(bundle)
        .or(info.display_name.clone())
        .unwrap_or_else(|| stem.clone());
    let mut aliases = Vec::new();
    for a in [Some(stem), info.name, info.display_name, info.executable]
        .into_iter()
        .flatten()
    {
        if a != name && !aliases.contains(&a) {
            aliases.push(a);
        }
    }
    Some(AppEntry {
        key: info
            .id
            .unwrap_or_else(|| bundle.display().to_string())
            .to_lowercase(),
        name,
        aliases,
        subtitle: bundle.display().to_string(),
        launch: LaunchTarget::Path(bundle.to_path_buf()),
        location: Some(bundle.to_path_buf()),
    })
}

/// The bundle's icon at `px` × `px`, as straight-alpha BGRA.
/// On macOS, uses `NSWorkspace.iconForFile:` so apps whose icon lives only in
/// an `Assets.car` asset catalog (Safari, System Settings, modern macOS apps)
/// get their real icon. Falls back to reading `.icns` directly.
pub fn icon(target: &LaunchTarget, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let LaunchTarget::Path(bundle) = target else {
        return None;
    };
    #[cfg(target_os = "macos")]
    if let Some(img) = workspace_icon(bundle, px) {
        return Some(img);
    }
    icns_icon(bundle, px)
}

#[cfg(target_os = "macos")]
fn workspace_icon(bundle: &Path, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSWorkspace;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        CGImageByteOrderInfo, CGInterpolationQuality,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    if px == 0 || !bundle.exists() {
        return None;
    }
    autoreleasepool(|_| {
        let ns_path = NSString::from_str(&bundle.to_string_lossy());
        let ns_image = NSWorkspace::sharedWorkspace().iconForFile(&ns_path);
        let mut proposed = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(px as f64, px as f64));
        let cg_image =
            unsafe { ns_image.CGImageForProposedRect_context_hints(&mut proposed, None, None) }?;
        let (w, h) = (px as usize, px as usize);
        let mut buf = vec![0u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb()?;
        // Little-endian premultiplied ARGB in memory is [B, G, R, A].
        let bitmap_info =
            CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0;
        let ctx = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&space),
                bitmap_info,
            )
        }?;
        CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
        let src_w = CGImage::width(Some(&cg_image)) as f64;
        let src_h = CGImage::height(Some(&cg_image)) as f64;
        let dest = if src_w > 0.0 && src_h > 0.0 {
            let scale = (px as f64 / src_w).min(px as f64 / src_h);
            let dw = src_w * scale;
            let dh = src_h * scale;
            CGRect::new(
                CGPoint::new((px as f64 - dw) / 2.0, (px as f64 - dh) / 2.0),
                CGSize::new(dw, dh),
            )
        } else {
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(px as f64, px as f64))
        };
        CGContext::draw_image(Some(&ctx), dest, Some(&cg_image));
        drop(ctx);

        // Premultiplied BGRA → straight-alpha BGRA; reject blank images.
        let mut any = false;
        for p in buf.as_chunks_mut::<4>().0 {
            let a = p[3] as u32;
            if a > 0 {
                any = true;
                for c in &mut p[..3] {
                    *c = (*c as u32 * 255 + a / 2)
                        .checked_div(a)
                        .unwrap_or(0)
                        .min(255) as u8;
                }
            }
        }
        any.then_some((w, h, buf))
    })
}

fn icns_icon(bundle: &Path, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let file = bundle_info(bundle)
        .icon_file
        .unwrap_or_else(|| "AppIcon".into());
    let file = if file.ends_with(".icns") {
        file
    } else {
        format!("{file}.icns")
    };
    let family = icns::IconFamily::read(std::io::BufReader::new(
        std::fs::File::open(bundle.join("Contents/Resources").join(file)).ok()?,
    ))
    .ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_bundle_ids_are_disambiguated_by_path() {
        let mut entries = vec![
            AppEntry {
                key: "com.adobe.illustrator".into(),
                name: "Adobe Illustrator 2026".into(),
                aliases: vec![],
                subtitle: "/Applications/Adobe Illustrator 2026/Adobe Illustrator.app".into(),
                launch: LaunchTarget::Path(
                    "/Applications/Adobe Illustrator 2026/Adobe Illustrator.app".into(),
                ),
                location: Some("/Applications/Adobe Illustrator 2026/Adobe Illustrator.app".into()),
            },
            AppEntry {
                key: "com.adobe.illustrator".into(),
                name: "Adobe Illustrator 2027".into(),
                aliases: vec![],
                subtitle: "/Applications/Adobe Illustrator 2027/Adobe Illustrator.app".into(),
                launch: LaunchTarget::Path(
                    "/Applications/Adobe Illustrator 2027/Adobe Illustrator.app".into(),
                ),
                location: Some("/Applications/Adobe Illustrator 2027/Adobe Illustrator.app".into()),
            },
            AppEntry {
                key: "com.apple.safari".into(),
                name: "Safari".into(),
                aliases: vec![],
                subtitle: "/Applications/Safari.app".into(),
                launch: LaunchTarget::Path("/Applications/Safari.app".into()),
                location: Some("/Applications/Safari.app".into()),
            },
        ];
        disambiguate_keys(&mut entries);
        assert_eq!(
            entries[0].key,
            "com.adobe.illustrator::/applications/adobe illustrator 2026/adobe illustrator.app"
        );
        assert_eq!(
            entries[1].key,
            "com.adobe.illustrator::/applications/adobe illustrator 2027/adobe illustrator.app"
        );
        assert_eq!(entries[2].key, "com.apple.safari");
    }
}
