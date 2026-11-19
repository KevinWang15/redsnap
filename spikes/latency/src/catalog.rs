//! A fixture app catalog (2,000 names, Latin and CJK) with generated icons,
//! and the matcher the real engine will use (nucleo), so per-keystroke cost
//! is realistic.

use std::sync::Arc;

use gpui::RenderImage;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

pub struct App {
    pub name: String,
    pub subtitle: String,
    pub icon: usize,
    haystack: Utf32String,
}

pub struct Hit {
    pub app: usize,
    pub score: u32,
}

pub struct Catalog {
    pub apps: Vec<App>,
    pub icons: Vec<Arc<RenderImage>>,
    matcher: Matcher,
}

const KNOWN: &[(&str, &str)] = &[
    ("Safari", "/Applications/Safari.app"),
    ("System Settings", "/Applications/System Settings.app"),
    ("Spell Word...", "RedSnap Dictionary & Lookup"),
    ("Bitwig Studio", "/Applications/Bitwig Studio.app"),
    ("Visual Studio Code", "/Applications/Visual Studio Code.app"),
    ("Slack", "/Applications/Slack.app"),
    ("Spotify", "/Applications/Spotify.app"),
    ("Sublime Text", "/Applications/Sublime Text.app"),
    ("Steam", "/Applications/Steam.app"),
    ("Terminal", "/System/Applications/Utilities/Terminal.app"),
    ("Google Chrome", "/Applications/Google Chrome.app"),
    ("Firefox", "/Applications/Firefox.app"),
    ("微信", "/Applications/WeChat.app"),
    ("网易云音乐", "/Applications/NeteaseMusic.app"),
    ("飞书", "/Applications/Lark.app"),
    ("钉钉", "/Applications/DingTalk.app"),
    ("计算器", "/System/Applications/Calculator.app"),
    ("Finder", "/System/Library/CoreServices/Finder.app"),
    ("Notes", "/System/Applications/Notes.app"),
    ("Calendar", "/System/Applications/Calendar.app"),
];

const WORDS_A: &[&str] = &[
    "Smart", "Quick", "Pro", "Studio", "Cloud", "Pixel", "Sound", "Data", "Note", "Code", "Photo", "Video", "Mail",
    "Task", "Time", "Web", "Net", "File", "Disk", "Color", "Light", "Dark", "Open", "Swift", "Rust", "Blue", "Red",
    "Green", "Micro", "Mega",
];
const WORDS_B: &[&str] = &[
    "Editor", "Viewer", "Manager", "Player", "Recorder", "Browser", "Studio", "Sync", "Backup", "Monitor", "Reader",
    "Writer", "Builder", "Desk", "Board", "Lab", "Box", "Kit", "Hub", "Flow", "Notes", "Paint", "Tracker", "Shell",
    "Console", "Mixer", "Lens", "Scan", "Clip", "Snap",
];
const WORDS_CJK: &[&str] = &["文档", "音乐", "视频", "照片", "笔记", "邮件", "日历", "地图", "翻译", "会议"];

impl Catalog {
    pub fn fixture() -> Self {
        let mut apps = Vec::new();
        let mut push = |name: String, subtitle: String| {
            let icon = apps.len() % ICON_COUNT;
            let haystack = Utf32String::from(name.as_str());
            apps.push(App { name, subtitle, icon, haystack });
        };
        for (name, sub) in KNOWN {
            push(name.to_string(), sub.to_string());
        }
        'outer: for (i, a) in WORDS_A.iter().enumerate() {
            for (j, b) in WORDS_B.iter().enumerate() {
                let name = if (i + j) % 7 == 0 {
                    format!("{a}{}", WORDS_CJK[(i * 3 + j) % WORDS_CJK.len()])
                } else {
                    format!("{a} {b}")
                };
                push(name.clone(), format!("/Applications/{name}.app"));
                push(format!("{name} {}", 2 + (i + j) % 9), format!("/Applications/{name} {}.app", 2 + (i + j) % 9));
                push(format!("{b} for {a}"), format!("/Applications/{b} for {a}.app"));
                if i * WORDS_B.len() + j >= 700 {
                    break 'outer;
                }
            }
        }
        apps.truncate(2000);
        Catalog { apps, icons: icons(), matcher: Matcher::new(Config::DEFAULT) }
    }

    /// The best `limit` matches for `query`, best first. An empty query lists
    /// the catalog in order (stand-in for "most used").
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<Hit> {
        if query.trim().is_empty() {
            return (0..limit.min(self.apps.len())).map(|app| Hit { app, score: 0 }).collect();
        }
        let pattern = Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);
        let mut hits: Vec<Hit> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(i, a)| pattern.score(a.haystack.slice(..), &mut self.matcher).map(|score| Hit { app: i, score }))
            .collect();
        let k = limit.min(hits.len());
        if k > 0 && k < hits.len() {
            hits.select_nth_unstable_by(k - 1, |a, b| b.score.cmp(&a.score).then(a.app.cmp(&b.app)));
            hits.truncate(k);
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.app.cmp(&b.app)));
        hits
    }
}

const ICON_COUNT: usize = 24;
/// Icons are drawn at 38 logical px; 96 px covers 2.5× displays.
const ICON_PX: u32 = 96;

/// Rounded-square icons with a diagonal gradient and a light disc, in BGRA
/// (the byte order gpui uploads).
fn icons() -> Vec<Arc<RenderImage>> {
    (0..ICON_COUNT)
        .map(|i| {
            let hue = i as f32 / ICON_COUNT as f32;
            let (r1, g1, b1) = hsl(hue, 0.70, 0.55);
            let (r2, g2, b2) = hsl((hue + 0.08) % 1.0, 0.75, 0.38);
            let n = ICON_PX as f32;
            let radius = n * 0.22;
            let mut buf = image::RgbaImage::new(ICON_PX, ICON_PX);
            for (x, y, px) in buf.enumerate_pixels_mut() {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                // Signed distance to a rounded square inset 2 px.
                let inset = 2.0;
                let half = n / 2.0 - inset;
                let (dx, dy) = ((fx - n / 2.0).abs() - (half - radius), (fy - n / 2.0).abs() - (half - radius));
                let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - radius;
                let alpha = (0.5 - outside).clamp(0.0, 1.0);
                let t = (fx + fy) / (2.0 * n);
                let mut c = [lerp(r1, r2, t), lerp(g1, g2, t), lerp(b1, b2, t)];
                let disc = ((fx - n / 2.0).powi(2) + (fy - n / 2.0).powi(2)).sqrt() - n * 0.2;
                let d = (0.5 - disc).clamp(0.0, 1.0) * 0.85;
                for ch in &mut c {
                    *ch = lerp(*ch, 255.0, d);
                }
                // BGRA, straight alpha.
                *px = image::Rgba([c[2] as u8, c[1] as u8, c[0] as u8, (alpha * 255.0) as u8]);
            }
            Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)]))
        })
        .collect()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_has_2000_apps_and_finds_safari_first() {
        let mut c = Catalog::fixture();
        assert_eq!(c.apps.len(), 2000);
        let hits = c.search("safari", 5);
        assert_eq!(c.apps[hits[0].app].name, "Safari");
        let hits = c.search("微信", 5);
        assert_eq!(c.apps[hits[0].app].name, "微信");
    }
}
