//! The matte glass behind the panel: what's on screen under the window,
//! blurred by us. The OS blurs (Windows acrylic) are far too strong to show
//! what's underneath and can't be tuned.
//!
//! The blur runs at a quarter of the resolution (box-downscale, then three box
//! blur passes, close to a Gaussian); gpui scales the small image back up with
//! linear filtering when drawing, which smooths it further.

use std::sync::Arc;

use gpui::RenderImage;

/// Downscale factor before blurring.
const SCALE: usize = 4;

/// Blurs a BGRA screen capture (`width` × `height`, rows tightly packed) with
/// roughly `radius` pixels of Gaussian blur at full resolution. Returns BGRA,
/// the byte order gpui uploads.
pub fn blur(width: usize, height: usize, bgra: &[u8], radius: f32) -> Arc<RenderImage> {
    let (w, h) = ((width / SCALE).max(1), (height / SCALE).max(1));
    // Box-downscale.
    let mut small = vec![0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            for dy in 0..SCALE {
                let row = ((y * SCALE + dy).min(height - 1)) * width;
                for dx in 0..SCALE {
                    let i = (row + (x * SCALE + dx).min(width - 1)) * 4;
                    acc[0] += bgra[i] as f32;
                    acc[1] += bgra[i + 1] as f32;
                    acc[2] += bgra[i + 2] as f32;
                }
            }
            let n = (SCALE * SCALE) as f32;
            let o = (y * w + x) * 3;
            small[o] = acc[0] / n;
            small[o + 1] = acc[1] / n;
            small[o + 2] = acc[2] / n;
        }
    }
    // Three box passes of radius r approximate a Gaussian with sigma ≈ r.
    let r = ((radius / SCALE as f32).round() as usize).max(1);
    let mut tmp = vec![0f32; small.len()];
    for _ in 0..3 {
        box_pass(&small, &mut tmp, w, h, r, true);
        box_pass(&tmp, &mut small, w, h, r, false);
    }
    let mut out = image::RgbaImage::new(w as u32, h as u32);
    for (i, px) in out.pixels_mut().enumerate() {
        let o = i * 3;
        *px = image::Rgba([small[o] as u8, small[o + 1] as u8, small[o + 2] as u8, 255]);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(out)]))
}

/// One running-sum box blur pass, horizontal or vertical, edges clamped.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let at = |line: usize, i: usize| if horizontal { (line * w + i) * 3 } else { (i * w + line) * 3 };
    let n = (2 * r + 1) as f32;
    for line in 0..lines {
        let mut sum = [0f32; 3];
        for k in 0..=2 * r {
            let i = at(line, k.saturating_sub(r).min(len - 1));
            for c in 0..3 {
                sum[c] += src[i + c];
            }
        }
        for i in 0..len {
            let o = at(line, i);
            for c in 0..3 {
                dst[o + c] = sum[c] / n;
            }
            let add = at(line, (i + r + 1).min(len - 1));
            let sub = at(line, i.saturating_sub(r));
            for c in 0..3 {
                sum[c] += src[add + c] - src[sub + c];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_flat_colors_and_shrinks_by_the_scale() {
        let (w, h) = (64, 32);
        let flat: Vec<u8> = (0..w * h).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let img = blur(w, h, &flat, 12.);
        let size = img.size(0);
        assert_eq!((size.width.0, size.height.0), (16, 8));
        let bytes = img.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[10, 20, 30, 255]);
    }
}
