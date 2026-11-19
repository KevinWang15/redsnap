//! Finding installed apps and their icons, per OS. Scans run on a background
//! thread; the result becomes a new immutable catalog.

use std::path::{Path, PathBuf};

use anyhow::Result;
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::AppEntry;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(target_os = "windows")]
use windows as imp;

/// Every installed app this OS lists, plus apps in `extra` folders.
pub fn scan(extra: &[PathBuf]) -> Result<Vec<AppEntry>> {
    let t = std::time::Instant::now();
    let apps = imp::scan(extra)?;
    log::info!("found {} apps in {:.0} ms", apps.len(), t.elapsed().as_secs_f64() * 1000.0);
    Ok(apps)
}

/// The icon for an app, as straight-alpha BGRA, about `px` pixels square.
/// Slow (disk, the shell): call on a background thread.
pub fn icon(target: &LaunchTarget, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    imp::icon(target, px)
}

/// Reads a PNG (or, on Linux, an SVG) and scales it to fit `px`, as BGRA.
#[allow(dead_code)]
fn decode_image(path: &Path, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let rgba = if path.extension().is_some_and(|x| x == "svg") {
        #[cfg(target_os = "linux")]
        {
            let data = std::fs::read(path).ok()?;
            let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
            let size = tree.size();
            let scale = px as f32 / size.width().max(size.height());
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
            // tiny-skia is premultiplied; undo it.
            let mut out = image::RgbaImage::new(px, px);
            for (o, p) in out.pixels_mut().zip(pixmap.pixels()) {
                let c = p.demultiply();
                *o = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
            }
            out
        }
        #[cfg(not(target_os = "linux"))]
        return None;
    } else {
        let img = image::open(path).ok()?.into_rgba8();
        if img.width() > px * 2 || img.height() > px * 2 {
            image::imageops::resize(&img, px, px, image::imageops::FilterType::Triangle)
        } else {
            img
        }
    };
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let mut data = rgba.into_raw();
    for p in data.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some((w, h, data))
}
