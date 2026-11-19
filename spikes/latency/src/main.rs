//! M0 latency spike (see PLAN.md §7): one pre-created, hidden search window,
//! shown by a global hotkey, with every show and keystroke timed to the frame
//! gpui presents.
//!
//! ```text
//! redsnap-spike [--paint=stock|now] [--glass=matte|blur|backdrop|transparent|opaque]
//!               [--alpha=0.6] [--blur=14]
//!               [--clip=dwm|region|none] [--hotkey=Ctrl+Alt+R]
//!               [--bench[=SHOWS]] [--onscreen] [--perf=perf.jsonl]
//! ```
//!
//! `--bench` drives itself: it shows the window, types queries through gpui's
//! own dispatch, hides it, and prints p50/p95/max. On Windows the bench window
//! stays cloaked (painted, never shown) and never takes focus, so nothing
//! reaches the desktop.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backdrop;
mod catalog;
mod launcher;
mod os;
mod perf;
mod query_field;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{
    AppContext, Application, Bounds, Keystroke, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    point, px, size,
};

use launcher::{Launcher, Options, PaintMode};

struct Args {
    paint: PaintMode,
    glass: WindowBackgroundAppearance,
    system_backdrop: bool,
    matte: bool,
    blur: f32,
    alpha: f32,
    clip: os::Clip,
    hotkey: String,
    bench: Option<usize>,
    onscreen: bool,
    perf: PathBuf,
}

fn parse_args() -> Args {
    let mut a = Args {
        paint: PaintMode::Now,
        glass: WindowBackgroundAppearance::Blurred,
        system_backdrop: false,
        matte: false,
        blur: 14.,
        alpha: 0.6,
        clip: os::Clip::Dwm,
        hotkey: "Ctrl+Alt+R".into(),
        bench: None,
        onscreen: false,
        perf: PathBuf::from("perf.jsonl"),
    };
    for arg in std::env::args().skip(1) {
        let (k, v) = arg.split_once('=').unwrap_or((arg.as_str(), ""));
        match k {
            "--paint" => a.paint = if v == "stock" { PaintMode::Stock } else { PaintMode::Now },
            "--glass" => {
                a.system_backdrop = v == "backdrop";
                a.matte = v == "matte";
                a.glass = match v {
                    // Matte draws its own blurred backdrop over a fully
                    // transparent window, with our own 20 px corners.
                    "transparent" | "matte" => WindowBackgroundAppearance::Transparent,
                    // gpui's own blur off; Windows 11's backdrop is set up in os::setup.
                    "opaque" | "backdrop" => WindowBackgroundAppearance::Opaque,
                    _ => WindowBackgroundAppearance::Blurred,
                }
            }
            "--clip" => {
                a.clip = match v {
                    "region" => os::Clip::Region,
                    "none" => os::Clip::None,
                    _ => os::Clip::Dwm,
                }
            }
            "--hotkey" => a.hotkey = v.into(),
            "--alpha" => a.alpha = v.parse().unwrap_or(a.alpha),
            "--blur" => a.blur = v.parse().unwrap_or(a.blur),
            "--bench" => a.bench = Some(v.parse().unwrap_or(30)),
            "--onscreen" => a.onscreen = true,
            "--perf" => a.perf = v.into(),
            _ => eprintln!("unknown argument {arg}"),
        }
    }
    a
}

#[cfg(windows)]
fn panel_radius(clip: os::Clip) -> f32 {
    if clip == os::Clip::Dwm { os::DWM_RADIUS } else { 20. }
}

#[cfg(not(windows))]
fn panel_radius(_: os::Clip) -> f32 {
    20.
}

const QUERIES: &[&str] = &["safari", "vsc", "sys", "微信", "bitwig", "code", "term", "photo", "studio", "red"];

fn main() {
    let args = parse_args();
    let mut logger = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,blade_graphics=warn,naga=warn"));
    // Release Windows builds have no console; log next to the perf file.
    if let Ok(file) = std::fs::File::create(args.perf.with_extension("log")) {
        logger.target(env_logger::Target::Pipe(Box::new(file)));
    }
    logger.init();
    let mode = match args.paint {
        PaintMode::Stock => "stock",
        PaintMode::Now => "now",
    };
    perf::init(&args.perf, mode);
    log::info!(
        "paint mode {mode}, glass {:?} (system backdrop {}), clip {:?}, alpha {}",
        args.glass,
        args.system_backdrop,
        args.clip,
        args.alpha
    );

    Application::new().run(move |cx| {
        query_field::bind_keys(cx);
        launcher::bind_keys(cx);
        let opts = Options {
            paint: args.paint,
            bench: args.bench.is_some(),
            cloaked: args.bench.is_some() && cfg!(windows) && !args.onscreen,
            radius: if args.matte { 20. } else { panel_radius(args.clip) },
            alpha: args.alpha,
            matte: args.matte,
            blur: args.blur,
        };
        let bounds = Bounds { origin: point(px(0.), px(0.)), size: size(px(launcher::WIDTH), px(launcher::HEIGHT)) };
        let created = Instant::now();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    focus: false,
                    show: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: args.glass,
                    app_id: Some("redsnap-spike".into()),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Launcher::new(opts, window, cx)),
            )
            .expect("open the search window");
        let native = handle
            .update(cx, |launcher, window, _| {
                let n = os::native(window);
                launcher.native = n;
                n
            })
            .ok()
            .flatten();
        perf::record("window_created", created.elapsed().as_secs_f64() * 1000.0);
        let entity = handle.update(cx, |_, _, cx| cx.entity().downgrade()).expect("launcher entity");
        // Matte: no Windows rounding or border; the panel draws its own corners.
        let clip = if args.matte { os::Clip::None } else { args.clip };
        let system_backdrop = args.system_backdrop;
        cx.spawn(async move |_| {
            if let Some(n) = native {
                os::setup(n, clip, if clip == os::Clip::None { 20. } else { panel_radius(clip) }, system_backdrop);
                // Created hidden; make sure it stays that way until the first show.
                os::hide(n);
            }
        })
        .detach();

        if let Some(shows) = args.bench {
            cx.spawn(async move |cx| {
                let executor = cx.background_executor().clone();
                let timer = |ms| executor.timer(Duration::from_millis(ms));
                timer(800).await;
                for i in 0..shows {
                    launcher::show(handle, entity.clone(), Instant::now(), cx).await;
                    timer(150).await;
                    let query = QUERIES[i % QUERIES.len()];
                    let keys: Vec<String> =
                        query.chars().map(|c| c.to_string()).chain(["backspace".into(), "backspace".into()]).collect();
                    for key in keys {
                        let ks = Keystroke::parse(&key).expect("keystroke");
                        let handled = cx.update_window(handle.into(), |_, window, cx| window.dispatch_keystroke(ks, cx));
                        log::debug!("key {key}: handled {handled:?}");
                        timer(60).await;
                    }
                    for _ in 0..3 {
                        let ks = Keystroke::parse("down").unwrap();
                        cx.update_window(handle.into(), |_, window, cx| window.dispatch_keystroke(ks, cx)).ok();
                        timer(60).await;
                    }
                    handle.update(cx, |launcher, _, cx| launcher.hide(cx)).ok();
                    timer(120).await;
                }
                println!("{}presents: {}", perf::summary(), perf::presents());
                perf::write_summary();
                cx.update(|cx| cx.quit()).ok();
            })
            .detach();
            return;
        }

        // Interactive: the global hotkey toggles the window.
        let hotkey: HotKey = args.hotkey.parse().expect("hotkey");
        let manager = GlobalHotKeyManager::new().expect("hotkey manager");
        if let Err(e) = manager.register(hotkey) {
            log::error!("can't register {}: {e}", args.hotkey);
        }
        // Lives as long as the app.
        Box::leak(Box::new(manager));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Instant>();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                let _ = tx.unbounded_send(Instant::now());
            }
        }));
        log::info!("press {} to show the search window", args.hotkey);
        cx.spawn(async move |cx| {
            while let Some(at) = rx.next().await {
                let visible = handle.update(cx, |l, _, _| l.visible).unwrap_or(false);
                if visible {
                    handle.update(cx, |l, _, cx| l.hide(cx)).ok();
                } else {
                    launcher::show(handle, entity.clone(), at, cx).await;
                }
            }
        })
        .detach();
    });
}
