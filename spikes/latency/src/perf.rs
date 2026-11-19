//! Latency timeline: when an input or a show starts, when its results are
//! ready, and when gpui presents the frame that shows them.
//!
//! gpui calls [`on_present`] after every presented frame (the `PRESENT_HOOK`
//! patch in third_party/gpui). Measurements go to `perf.jsonl` and into an
//! in-memory list that [`summary`] turns into p50/p95/max.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Hotkey (or bench trigger) → first presented frame of the shown window.
    Show,
    /// A keystroke in the query field → presented frame with its results.
    Key,
}

struct Pending {
    kind: Kind,
    started: Instant,
    results: Option<f64>,
}

struct State {
    pending: Option<Pending>,
    samples: BTreeMap<&'static str, Vec<f64>>,
    out: Option<File>,
    mode: &'static str,
    presents: u64,
    path: std::path::PathBuf,
}

static STATE: Mutex<State> =
    Mutex::new(State { pending: None, samples: BTreeMap::new(), out: None, mode: "stock", presents: 0, path: std::path::PathBuf::new() });

pub fn init(path: &Path, mode: &'static str) {
    let mut s = STATE.lock().unwrap();
    s.path = path.to_path_buf();
    s.out = File::create(path).map_err(|e| log::warn!("perf file {}: {e}", path.display())).ok();
    s.mode = mode;
    gpui::PRESENT_HOOK.set(on_present).ok();
}

/// A show begins (the hotkey arrived). Replaces anything still pending.
pub fn show_started(at: Instant) {
    STATE.lock().unwrap().pending = Some(Pending { kind: Kind::Show, started: at, results: None });
}

/// A keystroke reached the query field. The first keystroke of a burst wins,
/// so a frame that shows several keystrokes is measured from the earliest.
pub fn input_started() {
    let mut s = STATE.lock().unwrap();
    if s.pending.is_none() {
        s.pending = Some(Pending { kind: Kind::Key, started: Instant::now(), results: None });
    }
}

/// The results for the pending input or show are computed.
pub fn results_ready() {
    let mut s = STATE.lock().unwrap();
    if let Some(p) = s.pending.as_mut()
        && p.results.is_none()
    {
        p.results = Some(ms(p.started));
    }
}

/// Records a measurement that isn't tied to a presented frame.
pub fn record(name: &'static str, value_ms: f64) {
    let mut s = STATE.lock().unwrap();
    write_line(&mut s, name, value_ms, None);
    s.samples.entry(name).or_default().push(value_ms);
}

pub fn presents() -> u64 {
    STATE.lock().unwrap().presents
}

fn on_present(drawn: std::time::Duration, presented: std::time::Duration) {
    let mut s = STATE.lock().unwrap();
    s.presents += 1;
    let (d, p) = (drawn.as_secs_f64() * 1000.0, presented.as_secs_f64() * 1000.0);
    s.samples.entry("frame_draw").or_default().push(d);
    s.samples.entry("frame_draw_present").or_default().push(p);
    // A frame presented before results were ready (a cursor blink, say) isn't
    // the frame we're waiting for.
    let Some(p) = s.pending.as_ref() else { return };
    let Some(results) = p.results else { return };
    let total = ms(p.started);
    let name = match p.kind {
        Kind::Show => "show_to_present",
        Kind::Key => "key_to_present",
    };
    s.pending = None;
    write_line(&mut s, name, total, Some(results));
    s.samples.entry(name).or_default().push(total);
    s.samples.entry(if name == "show_to_present" { "show_results" } else { "key_results" }).or_default().push(results);
}

fn write_line(s: &mut State, name: &str, value: f64, results: Option<f64>) {
    let mode = s.mode;
    if let Some(out) = s.out.as_mut() {
        let results = results.map(|r| format!(",\"results_ms\":{r:.3}")).unwrap_or_default();
        let _ = writeln!(out, "{{\"metric\":\"{name}\",\"mode\":\"{mode}\",\"ms\":{value:.3}{results}}}");
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// p50 / p95 / max per metric, as text.
pub fn summary() -> String {
    let s = STATE.lock().unwrap();
    if s.samples.is_empty() {
        return format!("mode: {}\nno samples\n", s.mode);
    }
    let mut out = format!("mode: {}\n", s.mode);
    for (name, values) in &s.samples {
        let mut v = values.clone();
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
        out += &format!(
            "{name:<18} n={:<4} p50={:>7.2} ms  p95={:>7.2} ms  max={:>7.2} ms\n",
            v.len(),
            at(0.5),
            at(0.95),
            v[v.len() - 1]
        );
    }
    out
}

/// Writes [`summary`] to `<perf file>.summary.txt`.
pub fn write_summary() {
    let text = format!("{}presents: {}\n", summary(), presents());
    let path = STATE.lock().unwrap().path.with_extension("summary.txt");
    if let Err(e) = std::fs::write(&path, text) {
        log::warn!("{}: {e}", path.display());
    }
}
