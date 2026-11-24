//! Writes packaging/icons/: redsnap.ico (16–256 px), redsnap.icns (16–1024 px),
//! and PNGs of the app icon.
//! `cargo run --release -p redsnap-platform --example make_icons`

use std::fs::File;
use std::path::Path;

use icns::{IconFamily, IconType, Image, PixelFormat};

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

    // ICNS for the macOS RedSnap.app bundle (1x and 2x Retina slots).
    let mut family = IconFamily::new();
    let slots: &[(u32, &[IconType])] = &[
        (16, &[IconType::RGBA32_16x16]),
        (32, &[IconType::RGBA32_16x16_2x, IconType::RGBA32_32x32]),
        (64, &[IconType::RGBA32_32x32_2x]),
        (128, &[IconType::RGBA32_128x128]),
        (
            256,
            &[IconType::RGBA32_128x128_2x, IconType::RGBA32_256x256],
        ),
        (
            512,
            &[IconType::RGBA32_256x256_2x, IconType::RGBA32_512x512],
        ),
        (1024, &[IconType::RGBA32_512x512_2x]),
    ];
    for &(size, types) in slots {
        let rgba = redsnap_platform::icon::app_icon_rgba(size);
        let img = Image::from_data(PixelFormat::RGBA, size, size, rgba)?;
        for &kind in types {
            family.add_icon_with_type(&img, kind)?;
        }
    }
    let mut icns_file = File::create(dir.join("redsnap.icns"))?;
    family.write(&mut icns_file)?;

    println!("wrote {}", dir.display());
    Ok(())
}
