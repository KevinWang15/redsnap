//! Running an action after the window has hidden. OS work runs on a
//! background thread; a failure is logged and shown the next time the window
//! opens.

use std::sync::Mutex;

use gpui::{App, ClipboardItem, Global};
use redsnap_core::item::{Action, InternalCommand};
use redsnap_platform::open;
use redsnap_platform::paths::Paths;

/// App-wide things actions need.
pub struct Shell {
    pub paths: Paths,
}

impl Global for Shell {}

static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// The last action failure, once.
pub fn take_error() -> Option<String> {
    LAST_ERROR.lock().unwrap().take()
}

fn fail(what: &str, e: anyhow::Error) {
    log::warn!("{what}: {e:#}");
    *LAST_ERROR.lock().unwrap() = Some(format!("{what} failed: {e:#}"));
}

fn background(what: &'static str, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) {
    std::thread::spawn(move || {
        if let Err(e) = f() {
            fail(what, e);
        }
    });
}

pub fn run(action: Action, cx: &mut App) {
    log::info!("run {action:?}");
    match action {
        Action::CopyText(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
        Action::Launch(target) => background("Opening the app", move || open::launch(&target)),
        Action::OpenUrl(url) => background("Opening the link", move || open::open_url(&url)),
        Action::OpenPath(path) => background("Opening the path", move || open::open_path(&path)),
        Action::RevealPath(path) => background("Showing the file", move || open::reveal(&path)),
        Action::System(cmd) => background("The system command", move || open::run_system(cmd)),
        Action::RedSnap(cmd) => {
            let paths = cx.global::<Shell>().paths.clone();
            match cmd {
                // The settings app comes in M4; until then, the file itself.
                InternalCommand::OpenSettings => {
                    let file = paths.config.join("config.json");
                    background("Opening the settings", move || open::open_path(&file));
                }
                InternalCommand::ReloadApps => crate::app::reload_apps(cx),
                InternalCommand::About => background("Opening RedSnap's data folder", move || open::open_path(&paths.data)),
                InternalCommand::Quit => {
                    crate::perf::write_summary();
                    cx.quit()
                }
            }
        }
    }
}
