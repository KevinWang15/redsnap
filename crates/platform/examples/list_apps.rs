//! Lists the apps RedSnap finds on this machine and how many icons load.
//! `cargo run -p redsnap-platform --example list_apps [-- --all]`

fn main() -> anyhow::Result<()> {
    let all = std::env::args().any(|a| a == "--all");
    let apps = redsnap_platform::apps::scan(&[])?;
    let t = std::time::Instant::now();
    let mut icons = 0;
    for (i, a) in apps.iter().enumerate() {
        let icon = redsnap_platform::apps::icon(&a.launch, 64);
        if icon.is_some() {
            icons += 1;
        }
        if all || i < 25 {
            let size = icon
                .map(|(w, h, _)| format!("{w}x{h}"))
                .unwrap_or_else(|| "-".into());
            println!(
                "{:<40} {:<8} aliases {:?}  [{}]",
                a.name, size, a.aliases, a.key
            );
        }
    }
    println!(
        "{} apps, {} icons in {:.0} ms ({:.1} ms each)",
        apps.len(),
        icons,
        t.elapsed().as_secs_f64() * 1000.0,
        t.elapsed().as_secs_f64() * 1000.0 / apps.len().max(1) as f64
    );
    Ok(())
}
