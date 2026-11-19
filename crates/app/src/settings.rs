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
}

/// The settings app's executable and its arguments. A development build sets
/// `REDSNAP_SETTINGS_BIN` (the Electron binary) and `REDSNAP_SETTINGS_ARGS`
/// (the app folder); a packaged one finds it next to RedSnap.
fn locate() -> Result<(PathBuf, Vec<String>)> {
    if let Some(bin) = std::env::var_os("REDSNAP_SETTINGS_BIN") {
        let args = std::env::var("REDSNAP_SETTINGS_ARGS").map(|a| a.split('|').map(str::to_string).collect()).unwrap_or_default();
        return Ok((bin.into(), args));
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("no folder for RedSnap's executable")?;
    let path = if cfg!(windows) {
        dir.join("settings").join("RedSnap Settings.exe")
    } else if cfg!(target_os = "macos") {
        dir.join("../Resources/settings/RedSnap Settings.app/Contents/MacOS/RedSnap Settings")
    } else {
        dir.join("settings").join("redsnap-settings")
    };
    if !path.exists() {
        bail!("the settings app isn't installed ({} is missing)", path.display());
    }
    Ok((path, Vec::new()))
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
        let (bin, args) = locate()?;
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
        cmd.args(&args).env("REDSNAP_IPC_PATH", &session.path).env("REDSNAP_IPC_TOKEN", &session.token);
        // Don't pass the token on; the settings app reads it once at start.
        let child = cmd.spawn().with_context(|| format!("starting {}", bin.display()))?;
        log::info!("settings app started (pid {})", child.id());
        self.session = Some(session);
        self.child = Some(child);
        Ok(())
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
