//! Asks this OS's file index, the way `f …` in the launcher does:
//! `cargo run -p redsnap-platform --example find_files -- report q3`
//! (or a folder to list: `--example find_files -- --list C:\Users`).

use std::sync::mpsc;
use std::time::{Duration, Instant};

use redsnap_core::files::{FileRequest, Outcome};
use redsnap_platform::files::{BACKEND, FileSearch};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match args.first().map(String::as_str) {
        Some("--list") => FileRequest::List(args.get(1).map_or(".".into(), Into::into)),
        _ => FileRequest::Search(args.join(" ")),
    };
    let (tx, rx) = mpsc::channel();
    let search = FileSearch::start(Box::new(move |r| {
        let _ = tx.send(r);
    }))?;
    let t = Instant::now();
    search.request(request);
    let r = rx.recv_timeout(Duration::from_secs(10))?;
    println!("{BACKEND}: {:?} in {:.1} ms", r.request, t.elapsed().as_secs_f64() * 1000.0);
    match r.outcome {
        Outcome::Hits(hits) => {
            for h in hits.iter().take(30) {
                println!("{} {}", if h.is_dir { "d" } else { "-" }, h.path.display());
            }
            println!("{} hits", hits.len());
        }
        other => println!("{other:?}"),
    }
    Ok(())
}
