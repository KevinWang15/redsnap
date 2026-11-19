//! Writes packaging/icons/: redsnap.ico (16–256 px) and PNGs of the app icon.
//! `cargo run -p redsnap-platform --example make_icons`

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons");
    std::fs::create_dir_all(&dir)?;
    let mut frames = Vec::new();
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let rgba = redsnap_platform::icon::app_icon_rgba(size);
        let img = image::RgbaImage::from_raw(size, size, rgba).unwrap();
        if size >= 64 {
            img.save(dir.join(format!("redsnap-{size}.png")))?;
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        frames.push((size, png));
    }
    // ICO with PNG-compressed frames (Windows Vista and later).
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, png) in &frames {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[s, s, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &frames {
        ico.extend_from_slice(png);
    }
    std::fs::write(dir.join("redsnap.ico"), ico)?;
    println!("wrote {}", dir.display());
    Ok(())
}
