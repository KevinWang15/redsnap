//! The RedSnap mark, drawn in code: four corner brackets around a red rounded
//! square (the logo button in docs/design.md). Used for the tray until there
//! is designed artwork.

/// Straight RGBA, `size` × `size`, antialiased by 4×4 supersampling.
pub fn logo_rgba(size: u32) -> Vec<u8> {
    let n = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let thick = (n * 0.09).max(1.5);
    let arm = n * 0.30;
    let margin = n * 0.08;
    let inner = n * 0.36;
    let radius = inner * 0.25;
    let coverage = |x: f32, y: f32| -> Option<[u8; 3]> {
        // Red square, centered.
        let (cx, cy) = ((x - n / 2.0).abs() - (inner / 2.0 - radius), (y - n / 2.0).abs() - (inner / 2.0 - radius));
        if (cx.max(0.0).powi(2) + cy.max(0.0).powi(2)).sqrt() + cx.max(cy).min(0.0) <= radius {
            return Some([0xE8, 0x34, 0x4E]);
        }
        // Brackets: in a corner box, on its outer edges.
        for (bx, by) in [(margin, margin), (n - margin - arm, margin), (margin, n - margin - arm), (n - margin - arm, n - margin - arm)] {
            if x >= bx && x <= bx + arm && y >= by && y <= by + arm {
                let left = bx < n / 2.0;
                let top = by < n / 2.0;
                let on_v = if left { x <= bx + thick } else { x >= bx + arm - thick };
                let on_h = if top { y <= by + thick } else { y >= by + arm - thick };
                if on_v || on_h {
                    return Some([0xFF, 0xFF, 0xFF]);
                }
            }
        }
        None
    };
    for py in 0..size {
        for px in 0..size {
            let (mut hits, mut rgb) = (0u32, [0u32; 3]);
            for sy in 0..4 {
                for sx in 0..4 {
                    if let Some(c) = coverage(px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0) {
                        hits += 1;
                        for i in 0..3 {
                            rgb[i] += c[i] as u32;
                        }
                    }
                }
            }
            if hits > 0 {
                let o = ((py * size + px) * 4) as usize;
                for i in 0..3 {
                    out[o + i] = (rgb[i] / hits) as u8;
                }
                out[o + 3] = (hits * 255 / 16) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn logo_has_red_center_and_transparent_edges() {
        let n = 32;
        let px = super::logo_rgba(n);
        let at = |x: u32, y: u32| &px[((y * n + x) * 4) as usize..][..4];
        assert_eq!(at(16, 16), &[0xE8, 0x34, 0x4E, 255]);
        assert_eq!(at(0, 16)[3], 0);
    }
}
