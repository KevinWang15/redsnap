//! App icons: loaded off the UI thread (the Windows shell takes ~40 ms per
//! icon), cached on disk as PNG, kept in memory once decoded. Visible rows ask
//! first; after a scan every app's icon is warmed in the background.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use gpui::{App, Global, RenderImage};
use redsnap_core::item::{ItemId, LaunchTarget};

/// Pixels requested: 38 logical px at up to 2.5× scale.
const PX: u32 = 96;

struct Request {
    id: ItemId,
    target: LaunchTarget,
    /// Changes when the app does (its file's mtime), so a reinstalled app's
    /// new icon replaces the cached one.
    version: String,
}

pub struct Icons {
    state: Mutex<State>,
    visible: Sender<Request>,
    background: Sender<Request>,
}

#[derive(Default)]
struct State {
    /// `None`: looked up, the app has no icon.
    loaded: HashMap<ItemId, Option<Arc<RenderImage>>>,
    pending: HashSet<ItemId>,
}

impl Global for Icons {}

fn version_of(target: &LaunchTarget) -> String {
    let file = match target {
        LaunchTarget::Path(p) => Some(p.clone()),
        LaunchTarget::Desktop { file, .. } => Some(file.clone()),
        LaunchTarget::AppsFolder(id) => Some(PathBuf::from(id)),
    };
    file.and_then(|f| std::fs::metadata(f).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(String::new, |d| d.as_secs().to_string())
}

fn cache_file(dir: &Path, id: &ItemId, version: &str) -> PathBuf {
    let h = format!("{}|{version}|{PX}", id.as_str())
        .bytes()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
    dir.join(format!("{h:016x}.png"))
}

fn to_image(w: usize, h: usize, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(buf)
    ])))
}

/// One icon: from the disk cache, or from the OS (then cached).
fn load(dir: &Path, r: &Request) -> Option<Arc<RenderImage>> {
    let file = cache_file(dir, &r.id, &r.version);
    if let Ok(img) = image::open(&file) {
        let rgba = img.into_rgba8();
        let (w, h) = (rgba.width() as usize, rgba.height() as usize);
        let mut px = rgba.into_raw();
        for p in px.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        return to_image(w, h, px);
    }
    let (w, h, bgra) = redsnap_platform::apps::icon(&r.target, PX)?;
    let mut rgba = bgra.clone();
    for p in rgba.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
        let tmp = file.with_extension("tmp");
        if img.save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
    to_image(w, h, bgra)
}

impl Icons {
    /// Starts the loader thread. `done` is called on it after each icon; the
    /// app turns that into a repaint.
    pub fn start(
        cache_dir: PathBuf,
        done: impl Fn(ItemId, Option<Arc<RenderImage>>) + Send + 'static,
    ) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        let (visible, visible_rx) = mpsc::channel();
        let (background, background_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("icons".into())
            .spawn(move || run(cache_dir, visible_rx, background_rx, done))
            .expect("icon thread");
        Icons {
            state: Mutex::new(State::default()),
            visible,
            background,
        }
    }

    /// The icon if it's loaded; otherwise asks for it (visible rows go first)
    /// and returns `None` for now.
    pub fn get(&self, id: &ItemId, target: &LaunchTarget) -> Option<Arc<RenderImage>> {
        let mut s = self.state.lock().unwrap();
        if let Some(icon) = s.loaded.get(id) {
            return icon.clone();
        }
        if s.pending.insert(id.clone()) {
            let _ = self.visible.send(Request {
                id: id.clone(),
                target: target.clone(),
                version: version_of(target),
            });
        }
        None
    }

    /// Loads icons for all of `apps` in the background, after anything visible.
    pub fn warm(&self, apps: impl IntoIterator<Item = (ItemId, LaunchTarget)>) {
        let mut s = self.state.lock().unwrap();
        for (id, target) in apps {
            if !s.loaded.contains_key(&id) && s.pending.insert(id.clone()) {
                let version = version_of(&target);
                let _ = self.background.send(Request {
                    id,
                    target,
                    version,
                });
            }
        }
    }

    pub fn finished(&self, id: ItemId, icon: Option<Arc<RenderImage>>) {
        let mut s = self.state.lock().unwrap();
        s.pending.remove(&id);
        s.loaded.insert(id, icon);
    }
}

fn run(
    dir: PathBuf,
    visible: Receiver<Request>,
    background: Receiver<Request>,
    done: impl Fn(ItemId, Option<Arc<RenderImage>>),
) {
    loop {
        // Visible rows always first.
        let r = match visible.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => match background.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {
                    match visible.recv_timeout(std::time::Duration::from_millis(200)) {
                        Ok(r) => r,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            },
        };
        let icon = load(&dir, &r);
        done(r.id, icon);
    }
}

/// Convenience for render code.
pub fn get(cx: &App, id: &ItemId, target: &LaunchTarget) -> Option<Arc<RenderImage>> {
    cx.try_global::<Icons>().and_then(|i| i.get(id, target))
}
