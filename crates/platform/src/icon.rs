//! The RedSnap mark, drawn in code: four corner brackets around a red rounded
//! square (docs/design.md). [`logo_rgba`] is the bare mark (the tray);
//! [`app_icon_rgba`] puts it on a dark rounded tile, the app icon for windows,
//! the taskbar and the executables, readable on light and dark backgrounds.

const RED: [f32; 3] = [232.0, 52.0, 78.0];

/// The bare mark, straight RGBA, `size` × `size`, transparent around it.
pub fn logo_rgba(size: u32) -> Vec<u8> {
    render(size, false)
}

/// The app icon: the mark on a dark rounded tile.
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    render(size, true)
}

/// Signed distance from (x, y) to a rounded square centered at (c, c) with
/// half-size `h` and corner radius `r` (negative inside).
fn rounded_square(x: f32, y: f32, c: f32, h: f32, r: f32) -> f32 {
    let (dx, dy) = ((x - c).abs() - (h - r), (y - c).abs() - (h - r));
    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r
}

/// Color and coverage at one sample point, front to back.
fn sample(x: f32, y: f32, n: f32, tile: bool) -> Option<[f32; 4]> {
    // The mark lives in a square inset from the tile's edges.
    let (o, m) = if tile { (n * 0.16, n * 0.68) } else { (0.0, n) };
    let (mx, my) = ((x - o) / m, (y - o) / m);
    // Mark geometry in a unit square (matching the 44 px design).
    let inside = (0.0..=1.0).contains(&mx) && (0.0..=1.0).contains(&my);
    if inside {
        // Red square: 18/44, radius 4/44.
        if rounded_square(mx, my, 0.5, 9.0 / 44.0, 4.0 / 44.0) <= 0.0 {
            return Some([RED[0], RED[1], RED[2], 1.0]);
        }
        // Brackets: 13/44 long, 2.5/44 thick (thicker when small), 4/44 in.
        let thick = (2.5 / 44.0f32).max(1.6 / m);
        let (inset, arm) = (4.0 / 44.0, 13.0 / 44.0);
        let near = |v: f32| (inset..=inset + arm).contains(&v);
        let far = |v: f32| (1.0 - inset - arm..=1.0 - inset).contains(&v);
        let edge = |v: f32| (inset..=inset + thick).contains(&v) || (1.0 - inset - thick..=1.0 - inset).contains(&v);
        let in_corner = (near(mx) || far(mx)) && (near(my) || far(my));
        if in_corner && (edge(mx) && (near(my) || far(my)) || edge(my) && (near(mx) || far(mx))) {
            // Only the outer edges of each corner box.
            let left = mx < 0.5;
            let top = my < 0.5;
            let on_v = if left { mx <= inset + thick } else { mx >= 1.0 - inset - thick };
            let on_h = if top { my <= inset + thick } else { my >= 1.0 - inset - thick };
            if on_v || on_h {
                return Some([255.0, 255.0, 255.0, 0.95]);
            }
        }
    }
    if tile {
        let d = rounded_square(x, y, n / 2.0, n / 2.0 - n * 0.02, n * 0.22);
        if d <= 0.0 {
            // A subtle top-to-bottom gradient, and a faint light edge.
            let t = y / n;
            let base = [27.0 + (13.0 - 27.0) * t, 33.0 + (16.0 - 33.0) * t, 48.0 + (23.0 - 48.0) * t];
            if d > -(n * 0.02).max(1.0) {
                return Some([base[0] + 40.0, base[1] + 40.0, base[2] + 44.0, 1.0]);
            }
            return Some([base[0], base[1], base[2], 1.0]);
        }
    }
    None
}

/// Straight RGBA, antialiased by 4×4 supersampling.
fn render(size: u32, tile: bool) -> Vec<u8> {
    let n = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut alpha) = ([0.0f32; 3], 0.0f32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let (x, y) = (px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0);
                    if let Some(c) = sample(x, y, n, tile) {
                        // Premultiplied accumulation; a mark sample on the tile
                        // blends with the tile under it.
                        let under = if c[3] < 1.0 && tile { sample_tile(x, y, n) } else { None };
                        let a = c[3];
                        let (rgb, total) = match under {
                            Some(u) => ([c[0] * a + u[0] * (1.0 - a), c[1] * a + u[1] * (1.0 - a), c[2] * a + u[2] * (1.0 - a)], 1.0),
                            None => ([c[0] * a, c[1] * a, c[2] * a], a),
                        };
                        for i in 0..3 {
                            acc[i] += rgb[i];
                        }
                        alpha += total;
                    }
                }
            }
            if alpha > 0.0 {
                let o = ((py * size + px) * 4) as usize;
                for i in 0..3 {
                    out[o + i] = (acc[i] / alpha).round().clamp(0.0, 255.0) as u8;
                }
                out[o + 3] = (alpha / 16.0 * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn sample_tile(x: f32, y: f32, n: f32) -> Option<[f32; 3]> {
    if rounded_square(x, y, n / 2.0, n / 2.0 - n * 0.02, n * 0.22) <= 0.0 {
        let t = y / n;
        Some([27.0 + (13.0 - 27.0) * t, 33.0 + (16.0 - 33.0) * t, 48.0 + (23.0 - 48.0) * t])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mark_and_app_icon() {
        let n = 64;
        let at = |px: &[u8], x: u32, y: u32| px[((y * n + x) * 4) as usize..][..4].to_vec();
        let logo = super::logo_rgba(n);
        assert_eq!(&at(&logo, 32, 32)[..3], &[232, 52, 78]);
        assert_eq!(at(&logo, 1, 32)[3], 0);
        let icon = super::app_icon_rgba(n);
        assert_eq!(&at(&icon, 32, 32)[..3], &[232, 52, 78]);
        // The tile is opaque between the mark and its edge; corners are clear.
        assert_eq!(at(&icon, 32, 6)[3], 255);
        assert_eq!(at(&icon, 0, 0)[3], 0);
    }
}
