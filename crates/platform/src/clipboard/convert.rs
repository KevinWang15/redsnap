//! Clipboard format conversions that don't need the OS: DIB ↔ PNG, the
//! `CF_HDROP` file list, CF_HTML to text. Kept here so they're tested on any
//! platform.

use std::path::PathBuf;

use redsnap_core::clipboard::Content;

pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let d = image::ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png)
        .into_dimensions()
        .ok()?;
    Some(d)
}

/// A packed DIB (header, optional masks, pixels) to PNG. Handles 24- and
/// 32-bit uncompressed and bitfield images, top-down or bottom-up; anything
/// else (palettes, RLE) returns `None` and the copy is reported unsupported.
pub fn dib_to_png(dib: &[u8]) -> Option<Content> {
    let u32_at = |o: usize| {
        dib.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let header = u32_at(0)? as usize;
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bits = u16::from_le_bytes([*dib.get(14)?, *dib.get(15)?]);
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)? as usize;
    if width <= 0 || height == 0 || !(header == 40 || header == 108 || header == 124) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let mut offset = header;
    // BI_BITFIELDS with a plain header: three masks follow it.
    if compression == 3 && header == 40 {
        offset += 12;
    }
    offset += colors_used * 4;
    if !(bits == 24 || bits == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let pixels = dib.get(offset..offset + stride * h)?;
    let mut out = image::RgbaImage::new(w as u32, h as u32);
    let mut any_alpha = false;
    for y in 0..h {
        let src_y = if height > 0 { h - 1 - y } else { y };
        let row = &pixels[src_y * stride..];
        for x in 0..w {
            let (b, g, r, a) = if bits == 32 {
                let p = &row[x * 4..x * 4 + 4];
                (p[0], p[1], p[2], p[3])
            } else {
                let p = &row[x * 3..x * 3 + 3];
                (p[0], p[1], p[2], 255)
            };
            any_alpha |= a != 0;
            out.put_pixel(x as u32, y as u32, image::Rgba([r, g, b, a]));
        }
    }
    // 32-bit DIBs from many apps leave alpha at 0: treat as opaque.
    if bits == 32 && !any_alpha {
        for p in out.pixels_mut() {
            p[3] = 255;
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(Content::Image {
        png,
        width: w as u32,
        height: h as u32,
    })
}

/// PNG to a bottom-up 32-bit `CF_DIB`, for apps that don't read PNG.
pub fn png_to_dib(png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = Vec::with_capacity(40 + w * h * 4);
    for v in [40u32, w as u32, h as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32);
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    Some(out)
}

/// `CF_HDROP` payload: a `DROPFILES` header and NUL-separated wide paths.
pub fn dropfiles(files: &[PathBuf]) -> Vec<u8> {
    // sizeof(DROPFILES): pFiles, pt (2 × i32), fNC, fWide.
    let header = 20;
    let mut out = vec![0u8; header];
    out[..4].copy_from_slice(&(header as u32).to_le_bytes());
    // fWide = TRUE, at offset 16 (after pFiles, pt.x, pt.y, fNC).
    out[16..20].copy_from_slice(&1u32.to_le_bytes());
    for f in files {
        for u in f.to_string_lossy().encode_utf16().chain(Some(0)) {
            out.extend_from_slice(&u.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Text of a CF_HTML clipboard payload: the fragment, tags removed.
pub fn strip_html(cf_html: &str) -> String {
    let body = cf_html
        .find("<!--StartFragment-->")
        .and_then(|s| {
            cf_html[s..]
                .find("<!--EndFragment-->")
                .map(|e| &cf_html[s + 20..s + e])
        })
        .unwrap_or(cf_html);
    let mut out = String::new();
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_round_trip_through_png() {
        // 2×1 red-and-blue image, opaque.
        let png = {
            let img =
                image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
            let mut v = Vec::new();
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
                .unwrap();
            v
        };
        let dib = png_to_dib(&png).unwrap();
        let Some(Content::Image {
            png: back,
            width,
            height,
        }) = dib_to_png(&dib)
        else {
            panic!("not an image")
        };
        assert_eq!((width, height), (2, 1));
        let img = image::load_from_memory(&back).unwrap().into_rgba8();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn zero_alpha_dibs_are_opaque_and_palettes_are_refused() {
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1; // width 1
        dib[8] = 1; // height 1
        dib[12] = 1; // planes
        dib[14] = 32; // bits
        dib.extend_from_slice(&[10, 20, 30, 0]);
        let Some(Content::Image { png, .. }) = dib_to_png(&dib) else {
            panic!()
        };
        assert_eq!(
            image::load_from_memory(&png)
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0)
                .0,
            [30, 20, 10, 255]
        );
        dib[14] = 8;
        assert!(dib_to_png(&dib).is_none());
    }

    #[test]
    fn cf_html_fragment_text() {
        let h = "Version:0.9\r\nStartHTML:0\r\n<html><body><!--StartFragment--><b>Hi</b> &amp; bye<!--EndFragment--></body></html>";
        assert_eq!(strip_html(h), "Hi & bye");
    }

    #[test]
    fn dropfiles_layout() {
        let d = dropfiles(&["C:\\a.txt".into()]);
        assert_eq!(u32::from_le_bytes([d[0], d[1], d[2], d[3]]), 20);
        assert_eq!(u32::from_le_bytes([d[16], d[17], d[18], d[19]]), 1);
        assert!(d.ends_with(&[0, 0, 0, 0]));
    }
}
