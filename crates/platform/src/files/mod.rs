//! Files, the OS side: one background thread that answers
//! [`FileRequest`]s, newest first. What the answers mean for search is in
//! `redsnap_core::files`; docs/files.md has the behavior.
//!
//! Each OS searches its own index, behind the same [`BACKEND`], [`SYNTAX`]
//! and `search` in its module:
//! - Windows: Everything, over its window-message IPC (`windows.rs`).
//! - macOS: Spotlight, through `mdfind` (`macos.rs`).
//! - Linux: `plocate`, or `locate` (`linux.rs`).
//!
//! Listing a folder is the same everywhere (here).

use std::path::Path;
use std::sync::mpsc;

use redsnap_core::files::{FileHit, FileRequest, FileResults, Outcome};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

/// The index's name, for the hint row.
pub const BACKEND: &str = imp::BACKEND;
/// What the index understands, for the hint row.
pub const SYNTAX: &str = imp::SYNTAX;

/// Results asked of the index; the launcher shows the best few of them.
const SEARCH_LIMIT: usize = 200;
/// Entries read from one folder.
const LIST_LIMIT: usize = 5000;

/// Sends requests to the files thread. Only the newest waiting request is
/// answered; older ones are dropped unanswered.
#[derive(Clone)]
pub struct FileSearch {
    tx: mpsc::Sender<FileRequest>,
}

impl FileSearch {
    /// Starts the thread. `done` is called on it with each answer.
    pub fn start(done: Box<dyn Fn(FileResults) + Send>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("files".into()).spawn(move || run(rx, done))?;
        Ok(FileSearch { tx })
    }

    pub fn request(&self, r: FileRequest) {
        let _ = self.tx.send(r);
    }
}

fn run(rx: mpsc::Receiver<FileRequest>, done: Box<dyn Fn(FileResults) + Send>) {
    while let Ok(mut request) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            request = newer;
        }
        let t = std::time::Instant::now();
        let outcome = match &request {
            FileRequest::Search(words) => imp::search(words, SEARCH_LIMIT),
            FileRequest::List(dir) => list(dir, LIST_LIMIT),
        };
        log::debug!("{request:?}: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        done(FileResults { request, outcome });
    }
}

/// A folder's entries, unsorted. Links count as what they point to.
fn list(dir: &Path, limit: usize) -> Outcome {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => return Outcome::Failed(format!("{}: {e}", dir.display())),
    };
    let hits = entries
        .flatten()
        .take(limit)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let meta = std::fs::metadata(&path).or_else(|_| e.metadata()).ok();
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let hidden = name.starts_with('.') || meta.as_ref().is_some_and(hidden_by_os);
            FileHit { path, name, is_dir, hidden }
        })
        .collect();
    Outcome::Hits(hits)
}

#[cfg(windows)]
fn hidden_by_os(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    m.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn hidden_by_os(_: &std::fs::Metadata) -> bool {
    false
}

/// For the index modules: hits for paths, skipping ones that are gone (an
/// index can be behind the disk).
#[cfg_attr(windows, allow(dead_code))]
fn hits_for(paths: impl IntoIterator<Item = std::path::PathBuf>) -> Vec<FileHit> {
    paths
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            let hidden = name.starts_with('.');
            Some(FileHit { name, is_dir: meta.is_dir(), hidden, path })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join(".hidden"), "x").unwrap();
        let Outcome::Hits(mut hits) = list(dir.path(), 100) else { panic!() };
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        let got: Vec<(&str, bool, bool)> = hits.iter().map(|h| (h.name.as_str(), h.is_dir, h.hidden)).collect();
        assert_eq!(got, [(".hidden", false, true), ("a.txt", false, false), ("sub", true, false)]);
        assert!(matches!(list(&dir.path().join("missing"), 100), Outcome::Failed(_)));
    }

    #[test]
    fn answers_the_newest_request() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let search = FileSearch::start(Box::new(move |r| tx.send(r).unwrap())).unwrap();
        search.request(FileRequest::List(dir.path().into()));
        let r = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(r.request, FileRequest::List(dir.path().into()));
        assert_eq!(r.outcome, Outcome::Hits(Vec::new()));
    }
}
