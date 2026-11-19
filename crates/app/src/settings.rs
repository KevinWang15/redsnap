//! Starting the settings app (Electron) and talking to it. It starts on
//! demand, gets a private channel (see `redsnap_platform::settings_ipc`), and
//! quits when that channel closes, so it never outlives RedSnap.

use std::path::PathBuf;
use std::process::{Child, Command};

use anyhow::{Context as _, Result, bail};
use futures::channel::mpsc::UnboundedSender;
use redsnap_platform::paths::Paths;
use redsnap_platform::settings_ipc::Session;
use serde_json::{Value, json};

use crate::app::Event;

#[derive(Default)]
pub struct SettingsHost {
    session: Option<Session>,
    child: Option<Child>,
    /// When the settings app was asked to start, and which one, for timing.
    started: Option<(std::time::Instant, Backend)>,
}

/// Which settings app runs: the system webview one (`redsnap-settings`, a few
/// MB) or the Electron one (~300 MB, Chromium included). RedSnap prefers the
/// webview when it's installed; `REDSNAP_SETTINGS_BACKEND=webview|electron`
/// picks one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Webview,
    Electron,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Webview => "webview",
            Backend::Electron => "electron",
        }
    }
}

/// Where each backend is installed next to RedSnap.
fn installed(backend: Backend, dir: &std::path::Path) -> PathBuf {
    match backend {
        Backend::Webview if cfg!(windows) => dir.join("settings-webview").join("redsnap-settings.exe"),
        Backend::Webview if cfg!(target_os = "macos") => dir.join("../Resources/settings-webview/redsnap-settings"),
        Backend::Webview => dir.join("settings-webview").join("redsnap-settings"),
        Backend::Electron if cfg!(windows) => dir.join("settings").join("RedSnap Settings.exe"),
        Backend::Electron if cfg!(target_os = "macos") => {
            dir.join("../Resources/settings/RedSnap Settings.app/Contents/MacOS/RedSnap Settings")
        }
        Backend::Electron => dir.join("settings").join("redsnap-settings"),
    }
}

/// The settings app's executable, its arguments, and which one it is. A
/// development build sets `REDSNAP_SETTINGS_BIN` (and `REDSNAP_SETTINGS_ARGS`,
/// `|`-separated); a packaged one finds it next to RedSnap.
fn locate() -> Result<(PathBuf, Vec<String>, Backend)> {
    let wanted = match std::env::var("REDSNAP_SETTINGS_BACKEND").as_deref() {
        Ok("electron") => Some(Backend::Electron),
        Ok("webview") => Some(Backend::Webview),
        _ => None,
    };
    if let Some(bin) = std::env::var_os("REDSNAP_SETTINGS_BIN") {
        let args = std::env::var("REDSNAP_SETTINGS_ARGS").map(|a| a.split('|').map(str::to_string).collect()).unwrap_or_default();
        let bin = PathBuf::from(bin);
        let guess = if bin.to_string_lossy().contains("electron") { Backend::Electron } else { Backend::Webview };
        return Ok((bin, args, wanted.unwrap_or(guess)));
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("no folder for RedSnap's executable")?;
    let order = match wanted {
        Some(b) => vec![b],
        None => vec![Backend::Webview, Backend::Electron],
    };
    for b in order {
        let path = installed(b, dir);
        if path.exists() {
            return Ok((path, Vec::new(), b));
        }
    }
    bail!("the settings app isn't installed next to {}", exe.display())
}

impl SettingsHost {
    /// Opens the settings app, or brings it forward if it's open.
    pub fn open(&mut self, paths: &Paths, events: &UnboundedSender<Event>) -> Result<()> {
        if let Some(child) = &mut self.child {
            if child.try_wait().ok().flatten().is_none() {
                // Running: bring it forward (or it's still starting up).
                #[cfg(windows)]
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(child.id());
                }
                self.push("focus", json!({}));
                return Ok(());
            }
            // It exited without connecting (or the close event is on its way).
            self.child = None;
            self.session = None;
        }
        let started = std::time::Instant::now();
        let (bin, args, backend) = locate()?;
        let requests = events.clone();
        let closed = events.clone();
        let session = Session::start(
            &paths.data,
            Box::new(move |method, params, reply| {
                let _ = requests.unbounded_send(Event::Settings(method, params, reply));
            }),
            Box::new(move || {
                let _ = closed.unbounded_send(Event::SettingsClosed);
            }),
        )?;
        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .env("REDSNAP_IPC_PATH", &session.path)
            .env("REDSNAP_IPC_TOKEN", &session.token)
            .env("REDSNAP_SETTINGS_DATA", paths.data.join("settings-webview"));
        // Don't pass the token on; the settings app reads it once at start.
        let child = cmd.spawn().with_context(|| format!("starting {}", bin.display()))?;
        log::info!("settings app started ({}, pid {})", backend.name(), child.id());
        self.started = Some((started, backend));
        self.session = Some(session);
        self.child = Some(child);
        Ok(())
    }

    /// The settings app's window is up: logs how long that took.
    pub fn shown(&mut self) -> Value {
        match self.started.take() {
            Some((at, backend)) => {
                let ms = at.elapsed().as_secs_f64() * 1000.0;
                log::info!("settings window shown {ms:.0} ms after it was requested ({})", backend.name());
                crate::perf::record(if backend == Backend::Webview { "settings_open_webview" } else { "settings_open_electron" }, ms);
                json!({ "ms": ms, "backend": backend.name() })
            }
            None => json!({}),
        }
    }

    /// The settings app's channel closed (its window closed, or it crashed).
    pub fn closed(&mut self) {
        self.session = None;
        if let Some(mut child) = self.child.take() {
            // Reap it without blocking the UI.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    /// RedSnap is quitting.
    pub fn close(&mut self) {
        self.session = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }

    pub fn push(&self, event: &str, params: Value) {
        if let Some(s) = &self.session {
            s.push(event, params);
        }
    }
}
