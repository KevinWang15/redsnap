//! The global shortcuts: RedSnap's and the clipboard's. A change registers
//! the new shortcut before releasing the old one, and rolls back if it's
//! taken, so a failed change never leaves RedSnap without a shortcut.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use futures::channel::mpsc::UnboundedSender;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::app::Event;

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    main: Option<HotKey>,
    clip: Option<HotKey>,
    /// The clipboard shortcut's id, read by the event handler.
    clip_id: Arc<AtomicU32>,
    suspended: bool,
}

fn parse(spec: &str, what: &str) -> Result<HotKey, String> {
    spec.parse::<HotKey>()
        .map_err(|e| format!("The {what} shortcut {spec:?} isn't valid ({e})."))
}

impl Hotkeys {
    pub fn new(events: UnboundedSender<Event>) -> Self {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| log::error!("hotkeys unavailable: {e}"))
            .ok();
        let clip_id = Arc::new(AtomicU32::new(0));
        let id = clip_id.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                let clip = id.load(Ordering::Relaxed);
                let event = if clip != 0 && e.id == clip {
                    Event::ClipHotkey(Instant::now())
                } else {
                    Event::Hotkey(Instant::now())
                };
                let _ = events.unbounded_send(event);
            }
        }));
        Hotkeys {
            manager,
            main: None,
            clip: None,
            clip_id,
            suspended: false,
        }
    }

    fn register(&self, key: HotKey, spec: &str, what: &str) -> Result<(), String> {
        let m = self
            .manager
            .as_ref()
            .ok_or("Global shortcuts aren't available on this system.")?;
        m.register(key)
            .map_err(|e| format!("The {what} shortcut {spec} is taken by another app ({e})."))
    }

    fn unregister(&self, key: Option<HotKey>) {
        if let (Some(m), Some(k)) = (self.manager.as_ref(), key) {
            let _ = m.unregister(k);
        }
    }

    /// At startup: registers each shortcut on its own; returns what failed.
    pub fn start(&mut self, main: &str, clip: Option<&str>) -> Vec<String> {
        let mut problems = Vec::new();
        match parse(main, "RedSnap").and_then(|k| self.register(k, main, "RedSnap").map(|_| k)) {
            Ok(k) => self.main = Some(k),
            Err(e) => problems.push(e),
        }
        if let Some(spec) = clip {
            match parse(spec, "clipboard")
                .and_then(|k| self.register(k, spec, "clipboard").map(|_| k))
            {
                Ok(k) => self.clip = Some(k),
                Err(e) => problems.push(e),
            }
        }
        self.publish();
        problems
    }

    fn publish(&self) {
        self.clip_id
            .store(self.clip.map_or(0, |k| k.id()), Ordering::Relaxed);
    }

    /// Changes both shortcuts. New ones are registered first; on failure
    /// everything stays as it was.
    pub fn apply(&mut self, main: &str, clip: Option<&str>) -> Result<(), String> {
        let new_main = parse(main, "RedSnap")?;
        let new_clip = clip.map(|c| parse(c, "clipboard")).transpose()?;
        if new_clip.is_some_and(|c| c == new_main) {
            return Err("RedSnap and the clipboard need different shortcuts.".into());
        }
        if self.suspended {
            self.main = Some(new_main);
            self.clip = new_clip;
            self.publish();
            return Ok(());
        }
        let mut added = Vec::new();
        for (key, spec, what) in [
            (Some(new_main), main, "RedSnap"),
            (new_clip, clip.unwrap_or(""), "clipboard"),
        ] {
            let Some(key) = key else { continue };
            if Some(key) == self.main || Some(key) == self.clip {
                continue; // already ours
            }
            if let Err(e) = self.register(key, spec, what) {
                for k in added {
                    self.unregister(Some(k));
                }
                return Err(e);
            }
            added.push(key);
        }
        for old in [self.main, self.clip] {
            if old.is_some() && old != Some(new_main) && old != new_clip {
                self.unregister(old);
            }
        }
        self.main = Some(new_main);
        self.clip = new_clip;
        self.publish();
        Ok(())
    }

    /// While a shortcut is being recorded in Settings: pressing the current
    /// shortcut should record it, not open RedSnap.
    pub fn suspend(&mut self) {
        if !self.suspended {
            self.unregister(self.main);
            self.unregister(self.clip);
            self.suspended = true;
        }
    }

    pub fn resume(&mut self) -> Vec<String> {
        if !self.suspended {
            return Vec::new();
        }
        self.suspended = false;
        let mut problems = Vec::new();
        for (key, what) in [(self.main, "RedSnap"), (self.clip, "clipboard")] {
            if let Some(k) = key
                && let Err(e) = self.register(k, &format!("{k:?}"), what)
            {
                problems.push(e);
            }
        }
        problems
    }

    /// Whether `spec` could be used: valid, and not taken by another app.
    /// RedSnap's own current shortcuts count as available.
    pub fn test(&self, spec: &str) -> Result<(), String> {
        let key = parse(spec, "")?;
        if Some(key) == self.main || Some(key) == self.clip {
            return Ok(());
        }
        self.register(key, spec, "")?;
        self.unregister(Some(key));
        Ok(())
    }
}
