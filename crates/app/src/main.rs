//! RedSnap.
//!
//! ```text
//! redsnap                 start (or show the running one)
//! redsnap toggle|show|settings|quit
//!                         tell the running RedSnap (bind `redsnap toggle` to a
//!                         desktop shortcut where apps can't grab keys: Wayland)
//! redsnap --background    start hidden (start at login)
//! redsnap --profile=DIR   keep config and data under DIR (testing)
//! redsnap --perf=FILE     record show/keystroke latency to FILE
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

/// A key binding with the platform's command modifier: Cmd on macOS, Ctrl
/// elsewhere (`secondary!("enter")`).
macro_rules! secondary {
    ($key:literal) => {
        if cfg!(target_os = "macos") { concat!("cmd-", $key) } else { concat!("ctrl-", $key) }
    };
}

mod app;
mod assets;
mod execute;
mod hotkeys;
mod icons;
mod launcher;
mod perf;
mod settings;

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use redsnap_platform::instance::{self, Acquired, CliCommand};
use redsnap_platform::paths::Paths;

struct Args {
    command: Option<CliCommand>,
    background: bool,
    profile: Option<PathBuf>,
    perf: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args { command: None, background: false, profile: None, perf: None };
    for arg in std::env::args().skip(1) {
        let (k, v) = arg.split_once('=').unwrap_or((arg.as_str(), ""));
        match k {
            "toggle" => a.command = Some(CliCommand::Toggle),
            "show" => a.command = Some(CliCommand::Show),
            "settings" => a.command = Some(CliCommand::Settings),
            "quit" => a.command = Some(CliCommand::Quit),
            "--background" => a.background = true,
            "--profile" => a.profile = Some(v.into()),
            "--perf" => a.perf = Some(v.into()),
            _ => anyhow::bail!("unknown argument {arg:?}"),
        }
    }
    Ok(a)
}

/// Logs to `<data>/logs/redsnap.log` (the previous run's log is kept as
/// `redsnap.log.1`) and to stderr in debug builds.
fn init_logging(logs: &Path) {
    let path = logs.join("redsnap.log");
    let _ = std::fs::rename(&path, logs.join("redsnap.log.1"));
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,blade_graphics=warn,naga=warn"));
    if let Ok(file) = File::create(&path) {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    builder.init();
}

/// Appends panics, with a backtrace, to `<data>/logs/crash.log`.
fn record_panics(logs: &Path) {
    let crash = logs.join("crash.log");
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&crash) {
            let _ = writeln!(
                f,
                "--- {:?} RedSnap {}\n{info}\n{}",
                std::time::SystemTime::now(),
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture()
            );
        }
        default(info);
    }));
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let paths = Paths::resolve(args.profile.clone())?;

    let (tx, rx) = futures::channel::mpsc::unbounded();
    let cli_tx = tx.clone();
    match instance::acquire(&paths.data, move |c| {
        let _ = cli_tx.unbounded_send(app::Event::Cli(c));
    })? {
        Acquired::Secondary(client) => {
            // Already running: pass the command on (plain `redsnap` shows it).
            return client.send(args.command.unwrap_or(CliCommand::Show)).context("talking to the running RedSnap");
        }
        Acquired::Primary => {}
    }
    // Only the running RedSnap logs; a second launch mustn't rotate its log.
    init_logging(&paths.logs());
    record_panics(&paths.logs());
    if matches!(args.command, Some(CliCommand::Quit)) {
        return Ok(());
    }
    if let Some(p) = &args.perf {
        perf::init(p, "now");
    }
    log::info!("RedSnap {} starting; data in {}", env!("CARGO_PKG_VERSION"), paths.data.display());
    app::run(paths, app::Options { background: args.background }, tx, rx)
}
