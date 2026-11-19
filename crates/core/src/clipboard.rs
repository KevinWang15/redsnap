//! Clipboard history (docs/clipboard.md): the store and its search.
//!
//! Two rules shape this module:
//! - every copy is accounted for: it becomes an entry, moves an existing
//!   entry to the top (a duplicate), or becomes a *skipped* entry that says
//!   why it wasn't kept;
//! - search is literal: an entry matches only if it contains every typed
//!   word. No fuzzy matching.
//!
//! One thread owns the SQLite database. The in-memory list is replaced only
//! after a write commits.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use anyhow::{Context as _, Result, bail};
use arc_swap::ArcSwap;
use rusqlite::{Connection, OptionalExtension, params};

use crate::text;

const SCHEMA_VERSION: i64 = 1;
const DAY_MS: i64 = 86_400_000;
/// Skipped entries are kept a week: long enough to notice, short enough to
/// not clutter.
const SKIPPED_KEEP_MS: i64 = 7 * DAY_MS;

/// What was copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// Plain text, with the HTML the source app also offered.
    Text { text: String, html: Option<String> },
    /// A PNG (images are stored as PNG whatever format they were copied in).
    Image { png: Vec<u8>, width: u32, height: u32 },
    /// Files copied in Explorer / Finder / a file manager.
    Files(Vec<PathBuf>),
}

impl Content {
    fn kind(&self) -> Kind {
        match self {
            Content::Text { .. } => Kind::Text,
            Content::Image { .. } => Kind::Image,
            Content::Files(_) => Kind::Files,
        }
    }

    fn size(&self) -> usize {
        match self {
            Content::Text { text, html } => text.len() + html.as_ref().map_or(0, String::len),
            Content::Image { png, .. } => png.len(),
            Content::Files(f) => f.iter().map(|p| p.as_os_str().len()).sum(),
        }
    }

    /// The identity used to find duplicates: the text itself, the PNG bytes'
    /// hash, the file list.
    fn dedup_key(&self) -> String {
        match self {
            Content::Text { text, .. } => format!("t:{text}"),
            Content::Image { png, .. } => format!("i:{:016x}", fnv(png)),
            Content::Files(f) => {
                format!("f:{}", f.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\u{1}"))
            }
        }
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Text,
    Image,
    Files,
    /// A copy that wasn't kept; the entry says why.
    Skipped,
}

impl Kind {
    fn code(self) -> i64 {
        match self {
            Kind::Text => 0,
            Kind::Image => 1,
            Kind::Files => 2,
            Kind::Skipped => 3,
        }
    }

    fn from_code(c: i64) -> Kind {
        match c {
            1 => Kind::Image,
            2 => Kind::Files,
            3 => Kind::Skipped,
            _ => Kind::Text,
        }
    }
}

/// Why a copy wasn't kept. Shown to the user as-is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The source app marked it private (a password manager, say); names the marker.
    Private(String),
    ExcludedApp,
    TooLarge { bytes: u64 },
    Whitespace,
    /// Nothing RedSnap can keep (only formats it doesn't read); lists them.
    Unsupported(String),
    /// The clipboard couldn't be read in time (another app held it).
    Unreadable(String),
    /// Copies that were replaced before they could be read (macOS polling).
    Replaced { count: u64 },
}

impl Skip {
    pub fn describe(&self) -> String {
        match self {
            Skip::Private(marker) => format!("marked private by the app ({marker})"),
            Skip::ExcludedApp => "the app is excluded in Settings".into(),
            Skip::TooLarge { bytes } => format!("too large ({})", human_bytes(*bytes)),
            Skip::Whitespace => "only spaces or line breaks".into(),
            Skip::Unsupported(formats) => format!("no text, image or files in it ({formats})"),
            Skip::Unreadable(why) => format!("couldn't be read: {why}"),
            Skip::Replaced { count } => {
                format!("{count} cop{} replaced before {} could be read", if *count == 1 { "y" } else { "ies" }, if *count == 1 { "it" } else { "they" })
            }
        }
    }
}

pub fn human_bytes(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// One copy as the capture side reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capture {
    Content(Content),
    Skipped(Skip),
}

/// Limits from the config.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_text_bytes: usize,
    pub max_image_bytes: usize,
    pub keep_days: u32,
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_text_bytes: 4 << 20, max_image_bytes: 64 << 20, keep_days: 30, max_entries: 5000 }
    }
}

/// An entry as listed and searched. Image bytes stay in the database and are
/// fetched for the preview; text is kept in memory for search.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Random, never reused.
    pub id: String,
    pub kind: Kind,
    pub created_at: i64,
    pub last_copied_at: i64,
    pub source_app: Option<String>,
    pub pinned: bool,
    /// Text entries: the text. Files: the paths, one per line. Images:
    /// "Image W×H". Skipped: the reason.
    pub text: String,
    /// `text`, folded (lowercase, accents removed) for search.
    folded: String,
    pub size_bytes: u64,
    pub image_size: Option<(u32, u32)>,
}

/// The history at one moment, newest first.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub entries: Vec<Entry>,
}

/// Which entries a search may return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Images,
    Files,
    Urls,
}

/// A search hit: the entry and the byte ranges in `entry.text` to highlight.
#[derive(Clone, Debug)]
pub struct Found {
    pub index: usize,
    pub highlights: Vec<std::ops::Range<usize>>,
}

/// `img: cat` → (Images, "cat").
pub fn parse_query(q: &str) -> (Filter, &str) {
    let t = q.trim_start();
    for (prefix, f) in [("img:", Filter::Images), ("file:", Filter::Files), ("url:", Filter::Urls)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return (f, rest.trim_start());
        }
    }
    (Filter::All, t)
}

impl History {
    /// Entries containing every word of `query` (case and accents ignored),
    /// newest first; pinned entries first when the query is empty. Skipped
    /// entries show only with an empty query or when the words match their
    /// reason or app, and only if `show_skipped`.
    pub fn search(&self, query: &str, show_skipped: bool, limit: usize) -> Vec<Found> {
        let (filter, rest) = parse_query(query);
        let words: Vec<String> = text::fold(rest).split_whitespace().map(str::to_string).collect();
        let mut out = Vec::new();
        let mut push = |i: usize, e: &Entry| {
            let ok_kind = match filter {
                Filter::All => e.kind != Kind::Skipped || show_skipped,
                Filter::Images => e.kind == Kind::Image,
                Filter::Files => e.kind == Kind::Files,
                Filter::Urls => e.kind == Kind::Text && is_url(&e.text),
            };
            if !ok_kind {
                return;
            }
            let app = e.source_app.as_deref().map(text::fold).unwrap_or_default();
            if words.iter().all(|w| e.folded.contains(w.as_str()) || app.contains(w.as_str())) {
                out.push(Found { index: i, highlights: highlights(&e.text, &words) });
            }
        };
        if words.is_empty() {
            for (i, e) in self.entries.iter().enumerate().filter(|(_, e)| e.pinned) {
                push(i, e);
            }
            for (i, e) in self.entries.iter().enumerate().filter(|(_, e)| !e.pinned) {
                push(i, e);
            }
        } else {
            for (i, e) in self.entries.iter().enumerate() {
                push(i, e);
            }
        }
        out.truncate(limit);
        out
    }
}

fn is_url(s: &str) -> bool {
    let t = s.trim();
    (t.starts_with("http://") || t.starts_with("https://")) && !t.contains(char::is_whitespace)
}

/// Where each word occurs in `text`, for highlighting. Exact positions need
/// folding to keep byte lengths; where it doesn't (accented text), nothing is
/// highlighted rather than the wrong thing. Only the first 64 KB is scanned.
fn highlights(text: &str, words: &[String]) -> Vec<std::ops::Range<usize>> {
    let head = &text[..floor_char(text, 64 * 1024)];
    let lower = head.to_lowercase();
    if lower.len() != head.len() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    for w in words {
        let mut from = 0;
        while let Some(i) = lower[from..].find(w.as_str()) {
            ranges.push(from + i..from + i + w.len());
            from += i + w.len().max(1);
            if ranges.len() > 200 {
                break;
            }
        }
    }
    ranges.sort_by_key(|r| r.start);
    ranges
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

enum Cmd {
    Add { capture: Capture, source_app: Option<String>, at: i64, reply: Option<mpsc::Sender<Outcome>> },
    /// RedSnap pasted an entry: it moves to the top.
    Touch { id: String, at: i64 },
    Delete(String),
    Pin { id: String, pinned: bool },
    /// Removes everything but pinned entries.
    Clear,
    Fetch { id: String, reply: mpsc::Sender<Option<Content>> },
    Flush(mpsc::Sender<()>),
}

/// What happened to a capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Added(String),
    /// Moved an existing entry to the top.
    Duplicate(String),
    Skipped(String),
}

/// The owner of the clipboard database. Cheap to clone.
#[derive(Clone)]
pub struct ClipStore {
    tx: mpsc::Sender<Cmd>,
    history: Arc<ArcSwap<History>>,
}

impl ClipStore {
    pub fn open(path: &Path, limits: Limits) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        init(&conn)?;
        let history = Arc::new(ArcSwap::from_pointee(load(&conn)?));
        let (tx, rx) = mpsc::channel();
        let shared = history.clone();
        thread::Builder::new().name("clipboard-store".into()).spawn(move || run(conn, rx, shared, limits))?;
        Ok(ClipStore { tx, history })
    }

    pub fn history(&self) -> Arc<History> {
        self.history.load_full()
    }

    /// Records a capture. Never blocks on the database.
    pub fn add(&self, capture: Capture, source_app: Option<String>, at: i64) {
        let _ = self.tx.send(Cmd::Add { capture, source_app, at, reply: None });
    }

    /// Records a capture and waits for the outcome (tests, the debug log).
    pub fn add_wait(&self, capture: Capture, source_app: Option<String>, at: i64) -> Option<Outcome> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Cmd::Add { capture, source_app, at, reply: Some(reply) }).ok()?;
        rx.recv().ok()
    }

    pub fn touch(&self, id: &str, at: i64) {
        let _ = self.tx.send(Cmd::Touch { id: id.into(), at });
    }

    pub fn delete(&self, id: &str) {
        let _ = self.tx.send(Cmd::Delete(id.into()));
    }

    pub fn pin(&self, id: &str, pinned: bool) {
        let _ = self.tx.send(Cmd::Pin { id: id.into(), pinned });
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Cmd::Clear);
    }

    /// The full content of an entry (image bytes, HTML). Blocks; call off the
    /// UI thread.
    pub fn fetch(&self, id: &str) -> Option<Content> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(Cmd::Fetch { id: id.into(), reply }).ok()?;
        rx.recv().ok().flatten()
    }

    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Cmd::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

fn new_id() -> String {
    // 128 random bits from the OS, via a fresh hasher seed per call.
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn init(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 0 && version != SCHEMA_VERSION {
        bail!("clipboard.db has schema version {version}, expected {SCHEMA_VERSION}");
    }
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS clip (
             id TEXT PRIMARY KEY,
             kind INTEGER NOT NULL,
             dedup_key TEXT,
             created_at INTEGER NOT NULL,
             last_copied_at INTEGER NOT NULL,
             source_app TEXT,
             pinned INTEGER NOT NULL DEFAULT 0,
             text TEXT NOT NULL,
             html TEXT,
             image_png BLOB,
             image_width INTEGER,
             image_height INTEGER,
             files TEXT,
             size_bytes INTEGER NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS clip_dedup ON clip (dedup_key) WHERE dedup_key IS NOT NULL;
         CREATE INDEX IF NOT EXISTS clip_last_copied ON clip (last_copied_at);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn entry_from_row(r: &rusqlite::Row) -> rusqlite::Result<Entry> {
    let text: String = r.get(6)?;
    let w: Option<u32> = r.get(8)?;
    let h: Option<u32> = r.get(9)?;
    Ok(Entry {
        id: r.get(0)?,
        kind: Kind::from_code(r.get(1)?),
        created_at: r.get(2)?,
        last_copied_at: r.get(3)?,
        source_app: r.get(4)?,
        pinned: r.get::<_, i64>(5)? != 0,
        folded: text::fold(&text),
        text,
        size_bytes: r.get::<_, i64>(7)? as u64,
        image_size: w.zip(h),
    })
}

const COLUMNS: &str = "id, kind, created_at, last_copied_at, source_app, pinned, text, size_bytes, image_width, image_height";

fn load(conn: &Connection) -> Result<History> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM clip ORDER BY last_copied_at DESC, created_at DESC"))?;
    let entries = stmt.query_map([], entry_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(History { entries })
}

fn list_text(content: &Content) -> String {
    match content {
        Content::Text { text, .. } => text.clone(),
        Content::Image { width, height, .. } => format!("Image {width}×{height}"),
        Content::Files(f) => f.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
    }
}

/// Applies the size and whitespace rules: what is kept, or why not.
fn check(capture: Capture, limits: &Limits) -> Capture {
    let Capture::Content(content) = capture else { return capture };
    match &content {
        Content::Text { text, .. } if text.trim().is_empty() => Capture::Skipped(Skip::Whitespace),
        Content::Text { .. } if content.size() > limits.max_text_bytes => {
            Capture::Skipped(Skip::TooLarge { bytes: content.size() as u64 })
        }
        Content::Image { png, .. } if png.len() > limits.max_image_bytes => {
            Capture::Skipped(Skip::TooLarge { bytes: png.len() as u64 })
        }
        _ => Capture::Content(content),
    }
}

fn add(conn: &mut Connection, capture: Capture, source_app: Option<String>, at: i64, limits: &Limits) -> Result<Outcome> {
    let tx = conn.transaction()?;
    let outcome = match check(capture, limits) {
        Capture::Skipped(skip) => {
            let id = new_id();
            tx.execute(
                "INSERT INTO clip (id, kind, created_at, last_copied_at, source_app, text, size_bytes)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 0)",
                params![id, Kind::Skipped.code(), at, source_app, skip.describe()],
            )?;
            Outcome::Skipped(id)
        }
        Capture::Content(content) => {
            let key = content.dedup_key();
            let existing: Option<String> =
                tx.query_row("SELECT id FROM clip WHERE dedup_key = ?1", params![key], |r| r.get(0)).optional()?;
            match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE clip SET last_copied_at = MAX(last_copied_at, ?2), source_app = COALESCE(?3, source_app) WHERE id = ?1",
                        params![id, at, source_app],
                    )?;
                    Outcome::Duplicate(id)
                }
                None => {
                    let id = new_id();
                    let (html, png, w, h, files) = match &content {
                        Content::Text { html, .. } => (html.clone(), None, None, None, None),
                        Content::Image { png, width, height } => (None, Some(png.clone()), Some(*width), Some(*height), None),
                        Content::Files(f) => (None, None, None, None, Some(serde_json::to_string(f)?)),
                    };
                    tx.execute(
                        "INSERT INTO clip (id, kind, dedup_key, created_at, last_copied_at, source_app, text, html,
                                           image_png, image_width, image_height, files, size_bytes)
                         VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                        params![
                            id,
                            content.kind().code(),
                            key,
                            at,
                            source_app,
                            list_text(&content),
                            html,
                            png,
                            w,
                            h,
                            files,
                            content.size() as i64
                        ],
                    )?;
                    Outcome::Added(id)
                }
            }
        }
    };
    prune(&tx, at, limits)?;
    tx.commit()?;
    Ok(outcome)
}

/// Retention, in the same transaction as every write: entries older than
/// `keep_days`, skipped entries older than a week, and the oldest beyond
/// `max_entries`. Pinned entries are always kept.
fn prune(tx: &rusqlite::Transaction, now: i64, limits: &Limits) -> Result<()> {
    tx.execute(
        "DELETE FROM clip WHERE pinned = 0 AND (last_copied_at < ?1 OR (kind = ?2 AND last_copied_at < ?3))",
        params![now - limits.keep_days as i64 * DAY_MS, Kind::Skipped.code(), now - SKIPPED_KEEP_MS],
    )?;
    tx.execute(
        "DELETE FROM clip WHERE id IN (SELECT id FROM clip WHERE pinned = 0 ORDER BY last_copied_at DESC LIMIT -1 OFFSET ?1)",
        params![limits.max_entries as i64],
    )?;
    Ok(())
}

fn fetch(conn: &Connection, id: &str) -> Result<Option<Content>> {
    conn.query_row(
        "SELECT kind, text, html, image_png, image_width, image_height, files FROM clip WHERE id = ?1",
        params![id],
        |r| {
            let kind = Kind::from_code(r.get(0)?);
            Ok(match kind {
                Kind::Text => Some(Content::Text { text: r.get(1)?, html: r.get(2)? }),
                Kind::Image => Some(Content::Image {
                    png: r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
                    width: r.get::<_, Option<u32>>(4)?.unwrap_or(0),
                    height: r.get::<_, Option<u32>>(5)?.unwrap_or(0),
                }),
                Kind::Files => {
                    let json: Option<String> = r.get(6)?;
                    Some(Content::Files(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()))
                }
                Kind::Skipped => None,
            })
        },
    )
    .optional()
    .map(Option::flatten)
    .map_err(Into::into)
}

fn run(mut conn: Connection, rx: mpsc::Receiver<Cmd>, history: Arc<ArcSwap<History>>, limits: Limits) {
    let reload = |conn: &Connection| match load(conn) {
        Ok(h) => history.store(Arc::new(h)),
        Err(e) => log::warn!("reloading clipboard history: {e:#}"),
    };
    for cmd in rx {
        match cmd {
            Cmd::Add { capture, source_app, at, reply } => match add(&mut conn, capture, source_app, at, &limits) {
                Ok(outcome) => {
                    log::debug!("clipboard: {outcome:?}");
                    reload(&conn);
                    if let Some(r) = reply {
                        let _ = r.send(outcome);
                    }
                }
                Err(e) => log::warn!("saving a clipboard entry failed: {e:#}"),
            },
            Cmd::Touch { id, at } => {
                if conn.execute("UPDATE clip SET last_copied_at = MAX(last_copied_at, ?2) WHERE id = ?1", params![id, at]).is_ok() {
                    reload(&conn);
                }
            }
            Cmd::Delete(id) => {
                if conn.execute("DELETE FROM clip WHERE id = ?1", params![id]).is_ok() {
                    reload(&conn);
                }
            }
            Cmd::Pin { id, pinned } => {
                if conn.execute("UPDATE clip SET pinned = ?2 WHERE id = ?1", params![id, pinned as i64]).is_ok() {
                    reload(&conn);
                }
            }
            Cmd::Clear => {
                if conn.execute("DELETE FROM clip WHERE pinned = 0", []).is_ok() {
                    reload(&conn);
                }
            }
            Cmd::Fetch { id, reply } => {
                let _ = reply.send(fetch(&conn, &id).unwrap_or_else(|e| {
                    log::warn!("reading a clipboard entry: {e:#}");
                    None
                }));
            }
            Cmd::Flush(reply) => {
                let _ = reply.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn store() -> (tempfile::TempDir, ClipStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = ClipStore::open(&dir.path().join("clipboard.db"), Limits::default()).unwrap();
        (dir, s)
    }

    fn text(t: &str) -> Capture {
        Capture::Content(Content::Text { text: t.into(), html: None })
    }

    #[test]
    fn every_copy_is_accounted_for() {
        let (_d, s) = store();
        assert!(matches!(s.add_wait(text("hello"), Some("Notepad".into()), NOW), Some(Outcome::Added(_))));
        assert!(matches!(s.add_wait(text("world"), None, NOW + 1), Some(Outcome::Added(_))));
        // The same text again moves it to the top instead of adding a row.
        assert!(matches!(s.add_wait(text("hello"), None, NOW + 2), Some(Outcome::Duplicate(_))));
        assert!(matches!(s.add_wait(text("   \n"), None, NOW + 3), Some(Outcome::Skipped(_))));
        let h = s.history();
        let texts: Vec<&str> = h.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["only spaces or line breaks", "hello", "world"]);
        assert_eq!(h.entries[0].kind, Kind::Skipped);
        assert_eq!(h.entries[1].source_app.as_deref(), Some("Notepad"));
    }

    #[test]
    fn search_is_literal() {
        let (_d, s) = store();
        for (i, t) in ["the quick brown fox", "quicksort in rust", "brown bear", "Café au lait"].iter().enumerate() {
            s.add_wait(text(t), None, NOW + i as i64);
        }
        let h = s.history();
        let found = |q: &str| h.search(q, false, 50).iter().map(|f| h.entries[f.index].text.clone()).collect::<Vec<_>>();
        assert_eq!(found("quick"), ["quicksort in rust", "the quick brown fox"]);
        assert_eq!(found("brown quick"), ["the quick brown fox"]);
        // Scattered letters don't match: q-k-f is in "quick fox" but isn't a word.
        assert!(found("qkf").is_empty());
        assert_eq!(found("cafe"), ["Café au lait"]);
        assert_eq!(found("").len(), 4);
        let hit = &h.search("rust", false, 1)[0];
        assert_eq!(&h.entries[hit.index].text[hit.highlights[0].clone()], "rust");
    }

    #[test]
    fn skipped_entries_follow_the_setting() {
        let (_d, s) = store();
        s.add_wait(text("kept"), None, NOW);
        s.add_wait(Capture::Skipped(Skip::Private("ExcludeClipboardContentFromMonitorProcessing".into())), Some("KeePassXC".into()), NOW + 1);
        let h = s.history();
        assert_eq!(h.search("", false, 10).len(), 1);
        assert_eq!(h.search("", true, 10).len(), 2);
        assert_eq!(h.search("keepass", true, 10).len(), 1);
    }

    #[test]
    fn images_and_files_round_trip_and_dedupe() {
        let (_d, s) = store();
        let img = Content::Image { png: vec![1, 2, 3], width: 2, height: 1 };
        let Some(Outcome::Added(id)) = s.add_wait(Capture::Content(img.clone()), None, NOW) else { panic!() };
        assert!(matches!(s.add_wait(Capture::Content(img.clone()), None, NOW + 1), Some(Outcome::Duplicate(_))));
        assert_eq!(s.fetch(&id), Some(img));
        let files = Content::Files(vec!["/tmp/a.txt".into(), "/tmp/b.png".into()]);
        let Some(Outcome::Added(fid)) = s.add_wait(Capture::Content(files.clone()), None, NOW + 2) else { panic!() };
        assert_eq!(s.fetch(&fid), Some(files));
        let h = s.history();
        assert_eq!(h.search("file: b.png", false, 10).len(), 1);
        assert_eq!(h.search("img:", false, 10).len(), 1);
    }

    #[test]
    fn retention_keeps_pinned_entries() {
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits { max_entries: 2, ..Limits::default() };
        let s = ClipStore::open(&dir.path().join("c.db"), limits).unwrap();
        let Some(Outcome::Added(first)) = s.add_wait(text("one"), None, NOW) else { panic!() };
        s.pin(&first, true);
        for (i, t) in ["two", "three", "four"].iter().enumerate() {
            s.add_wait(text(t), None, NOW + 1 + i as i64);
        }
        let texts: Vec<String> = s.history().entries.iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["four", "three", "one"]);
        // Old entries go after keep_days.
        s.add_wait(text("much later"), None, NOW + 40 * DAY_MS);
        let texts: Vec<String> = s.history().entries.iter().map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["much later", "one"]);
    }

    #[test]
    fn too_large_is_skipped_with_the_size() {
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits { max_text_bytes: 10, ..Limits::default() };
        let s = ClipStore::open(&dir.path().join("c.db"), limits).unwrap();
        s.add_wait(text("this is longer than ten bytes"), None, NOW);
        assert_eq!(s.history().entries[0].text, "too large (29 B)");
    }
}
