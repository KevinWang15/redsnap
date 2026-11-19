//! Watching the system clipboard, writing to it, and pasting into the app
//! that was in front (docs/clipboard.md). Every change the OS reports ends as
//! one [`ClipEvent`]: a capture, a skip with its reason, or RedSnap's own
//! write.

use redsnap_core::clipboard::{Capture, Content};

pub mod convert;
#[cfg(windows)]
mod windows;

#[derive(Clone, Debug)]
pub struct ClipEvent {
    /// Milliseconds since the Unix epoch.
    pub at: i64,
    /// The app that put it there (executable name on Windows), when known.
    pub source_app: Option<String>,
    pub kind: ClipEventKind,
}

#[derive(Clone, Debug)]
pub enum ClipEventKind {
    Captured(Capture),
    /// RedSnap wrote this (pasting an entry); carries the entry id.
    OwnWrite(String),
}

pub type OnClip = Box<dyn Fn(ClipEvent) + Send + 'static>;

/// Limits applied while reading, before anything large is copied out of the
/// clipboard.
#[derive(Clone, Copy, Debug)]
pub struct ReadLimits {
    pub max_text_bytes: usize,
    pub max_image_bytes: usize,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Watches the clipboard until dropped.
pub struct Watcher {
    #[cfg(windows)]
    inner: windows::Watcher,
}

impl Watcher {
    /// Starts watching; `on_clip` is called on a background thread. `None`
    /// where capture isn't implemented yet (macOS, Linux: later in M5).
    pub fn start(limits: ReadLimits, on_clip: OnClip) -> anyhow::Result<Option<Watcher>> {
        #[cfg(windows)]
        return Ok(Some(Watcher { inner: windows::Watcher::start(limits, on_clip)? }));
        #[cfg(not(windows))]
        {
            let _ = (limits, on_clip);
            Ok(None)
        }
    }

    /// Puts `content` on the clipboard, marked as RedSnap's own write for
    /// entry `id`, so the watcher doesn't record it as a new copy.
    pub fn write(&self, content: &Content, id: &str) -> anyhow::Result<()> {
        #[cfg(windows)]
        return self.inner.write(content, id);
        #[cfg(not(windows))]
        {
            let _ = (content, id);
            anyhow::bail!("writing the clipboard isn't implemented on this OS yet")
        }
    }
}

/// The window in front before RedSnap showed itself, to paste into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target(pub usize);

/// The current foreground window (call before RedSnap takes the foreground).
pub fn foreground() -> Option<Target> {
    #[cfg(windows)]
    return windows::foreground();
    #[cfg(not(windows))]
    None
}

/// Gives `target` the foreground back and sends Ctrl+V (Cmd+V on macOS).
/// Blocks for up to half a second waiting for the target; call off the UI
/// thread.
pub fn paste_into(target: Target) -> anyhow::Result<()> {
    #[cfg(windows)]
    return windows::paste_into(target);
    #[cfg(not(windows))]
    {
        let _ = target;
        anyhow::bail!("pasting isn't implemented on this OS yet")
    }
}
