//! Results and the typed actions they run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A stable identity for a result, taken from its source (an app's AUMID,
/// bundle id or desktop file id; a command name; a config id). Never a
/// database row id: those are reused.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ItemId(pub String);

impl ItemId {
    pub fn new(kind: &str, key: &str) -> Self {
        ItemId(format!("{kind}:{key}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which provider produced an item. The order is the tie-break priority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderId {
    Calculator,
    Open,
    Apps,
    Commands,
    Snippets,
    QuickLinks,
    WebSearch,
}

/// What to draw as an item's icon. The app resolves these to images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconRef {
    /// An app's own icon, looked up by its item id.
    App(ItemId),
    /// One of RedSnap's built-in icons.
    Builtin(&'static str),
}

/// How to start an app, per OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    /// Windows: an AppsFolder id (packaged AUMID or a Start-menu shortcut's id).
    AppsFolder(String),
    /// A path the OS opens (an `.exe`, `.lnk`, `.app` bundle).
    Path(PathBuf),
    /// Linux: a desktop file id (`org.gnome.Nautilus.desktop`) and its file.
    Desktop { id: String, file: PathBuf },
}

/// OS commands RedSnap can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemCommand {
    Lock,
    Sleep,
    LogOut,
    Restart,
    ShutDown,
    EmptyTrash,
}

impl SystemCommand {
    /// Commands that end the session ask for confirmation first.
    pub fn needs_confirmation(self) -> bool {
        matches!(self, SystemCommand::LogOut | SystemCommand::Restart | SystemCommand::ShutDown)
    }
}

/// RedSnap's own commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InternalCommand {
    OpenSettings,
    ReloadApps,
    About,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Launch(LaunchTarget),
    /// Only http(s) and a short list of OS settings schemes; checked again
    /// when the action runs.
    OpenUrl(String),
    OpenPath(PathBuf),
    RevealPath(PathBuf),
    CopyText(String),
    System(SystemCommand),
    RedSnap(InternalCommand),
    /// Types the snippet with this id into the app that was in front.
    Snippet(String),
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Launch(_) => "Open",
            Action::OpenUrl(_) => "Open",
            Action::OpenPath(_) => "Open",
            Action::RevealPath(_) => {
                if cfg!(target_os = "macos") {
                    "Show in Finder"
                } else if cfg!(windows) {
                    "Show in Explorer"
                } else {
                    "Show in folder"
                }
            }
            Action::CopyText(_) => "Copy",
            Action::Snippet(_) => "Insert",
            Action::System(_) | Action::RedSnap(_) => "Run",
        }
    }
}

/// URL schemes an `OpenUrl` action may open.
pub fn url_allowed(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    ["http://", "https://", "ms-settings:", "x-apple.systempreferences:"].iter().any(|p| lower.starts_with(p))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub provider: ProviderId,
    pub title: String,
    pub subtitle: String,
    pub icon: IconRef,
    /// Enter.
    pub primary: Action,
    /// Cmd/Ctrl+Enter.
    pub secondary: Option<Action>,
    /// Whether picking it teaches ranking (calculator results don't).
    pub learnable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_settings_urls_open() {
        assert!(url_allowed("https://example.com"));
        assert!(url_allowed("ms-settings:display"));
        assert!(!url_allowed("file:///etc/passwd"));
        assert!(!url_allowed("javascript:alert(1)"));
    }
}
