//! Starting the settings app (the system webview: crates/settings-host) and
//! talking to it. It starts on
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
    /// When the settings app was asked to start, for timing.
    started: Option<std::time::Instant>,
}

/// The settings app next to RedSnap: `settings/redsnap-settings` (with its
/// UI in `settings/web/`), inside the bundle's Resources on macOS. A
/// development build sets `REDSNAP_SETTINGS_BIN` (and `REDSNAP_SETTINGS_WEB`
/// for the UI folder, which the settings app reads itself).
fn locate() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("REDSNAP_SETTINGS_BIN") {
        return Ok(bin.into());
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("no folder for RedSnap's executable")?;
    let path = if cfg!(windows) {
        dir.join("settings").join("redsnap-settings.exe")
    } else if cfg!(target_os = "macos") {
        dir.join("../Resources/settings/redsnap-settings")
    } else {
        dir.join("settings").join("redsnap-settings")
    };
    if !path.exists() {
        bail!("the settings app isn't installed ({} is missing)", path.display());
    }
    Ok(path)
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
        let bin = locate()?;
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
        cmd.env("REDSNAP_IPC_PATH", &session.path)
            .env("REDSNAP_IPC_TOKEN", &session.token)
            .env("REDSNAP_SETTINGS_DATA", paths.data.join("settings-webview"));
        // Don't pass the token on; the settings app reads it once at start.
        let child = cmd.spawn().with_context(|| format!("starting {}", bin.display()))?;
        log::info!("settings app started (pid {})", child.id());
        self.started = Some(started);
        self.session = Some(session);
        self.child = Some(child);
        Ok(())
    }

    /// The settings app's window is up: logs how long that took.
    pub fn shown(&mut self) -> Value {
        match self.started.take() {
            Some(at) => {
                let ms = at.elapsed().as_secs_f64() * 1000.0;
                log::info!("settings window shown {ms:.0} ms after it was requested");
                crate::perf::record("settings_open", ms);
                json!({ "ms": ms })
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
