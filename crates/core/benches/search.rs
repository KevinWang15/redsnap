//! Per-keystroke search cost over 10,000 apps with mixed Latin and Chinese
//! names (PLAN.md §5.10: matching 10,000 items ≤ 2 ms).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::{AppCatalog, AppEntry, AppsProvider};
use redsnap_core::providers::{CalculatorProvider, CommandsProvider, OpenProvider, WebSearchProvider};
use redsnap_core::search::Engine;
use redsnap_core::usage::Usage;

fn main() {
    let words = ["Smart", "Quick", "Pro", "Studio", "Cloud", "Pixel", "Sound", "Data", "Note", "Code", "Photo", "Video"];
    let tails = ["Editor", "Viewer", "Manager", "Player", "Sync", "Monitor", "Reader", "Board", "Lab", "Hub"];
    let cjk = ["文档", "音乐", "视频", "照片", "笔记", "邮件", "日历", "地图", "翻译", "会议"];
    let mut entries = Vec::new();
    for i in 0..10_000 {
        let name = if i % 5 == 0 {
            format!("{}{}{}", words[i % words.len()], cjk[(i / 7) % cjk.len()], i)
        } else {
            format!("{} {} {}", words[i % words.len()], tails[(i / 3) % tails.len()], i)
        };
        entries.push(AppEntry {
            key: format!("app{i}"),
            subtitle: format!("/Applications/{name}.app"),
            launch: LaunchTarget::Path(format!("/Applications/{name}.app").into()),
            location: None,
            aliases: vec![format!("{name}.exe")],
            name,
        });
    }
    let built = Instant::now();
    let catalog = AppCatalog::build(entries, &BTreeMap::new(), &[]);
    println!("catalog of {} apps built in {:.1} ms", catalog.len(), built.elapsed().as_secs_f64() * 1000.0);
    let config = redsnap_core::config::Config::default();
    let mut engine = Engine::new(
        vec![
            Box::new(CalculatorProvider),
            Box::new(OpenProvider { home: None }),
            Box::new(AppsProvider::new(Arc::new(ArcSwap::from_pointee(catalog)))),
            Box::new(CommandsProvider::new(&[])),
            Box::new(WebSearchProvider::new(config.web_searches)),
        ],
        Default::default(),
    );
    let usage = Arc::new(Usage::default());
    for query in ["s", "st", "stu", "stud", "studio", "vsc", "yy", "yinyue", "音乐", "zzz", "1+2", ""] {
        let mut times: Vec<f64> = (0..50)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(engine.search(query, &usage, 0, 50));
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(f64::total_cmp);
        println!("{query:>8}: p50 {:.2} ms  p95 {:.2} ms", times[25], times[47]);
    }
}
