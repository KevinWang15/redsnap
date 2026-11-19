//! Learned ranking: which result you picked for which query.
//!
//! One thread owns the SQLite connection. Reads go through an immutable
//! snapshot ([`Usage`]) that is replaced only after a write commits, so the
//! ranking never shows something the database doesn't have.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use anyhow::{Context as _, Result, bail};
use arc_swap::ArcSwap;
use rusqlite::{Connection, params};

use crate::item::ItemId;
use crate::text;

/// Picks are remembered for every prefix of the query up to this many
/// characters: picking Chrome for "chr" teaches "c", "ch" and "chr".
const MAX_PREFIX_CHARS: usize = 16;
/// Counts stop growing here, so years of habit can still be overtaken.
const MAX_COUNT: u32 = 20;
/// Oldest rows go first beyond this.
const MAX_PICK_ROWS: i64 = 50_000;
const SCHEMA_VERSION: i64 = 1;
const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Use {
    pub count: u32,
    /// Milliseconds since the Unix epoch.
    pub last_used: i64,
}

/// A read-only view of the usage data at one moment.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// Folded query prefix → item → use.
    picks: HashMap<String, HashMap<ItemId, Use>>,
    items: HashMap<ItemId, Use>,
}

fn recency(now: i64, last: i64) -> f32 {
    match (now - last).max(0) {
        d if d < DAY_MS => 1.0,
        d if d < 7 * DAY_MS => 0.7,
        d if d < 30 * DAY_MS => 0.4,
        _ => 0.2,
    }
}

impl Usage {
    /// How strongly picks for this exact (folded) query favor `id`; 0 if never
    /// picked for it.
    pub fn query_boost(&self, folded_query: &str, id: &ItemId, now: i64) -> u32 {
        let key: String = folded_query.chars().take(MAX_PREFIX_CHARS).collect();
        self.picks.get(&key).and_then(|m| m.get(id)).map_or(0, |u| {
            (u.count as f32 * 100.0 * recency(now, u.last_used)) as u32
        })
    }

    /// A small bonus for items used often and recently, whatever the query.
    /// At most 150, well below the gap between match classes.
    pub fn global(&self, id: &ItemId, now: i64) -> u32 {
        self.items.get(id).map_or(0, |u| {
            (u.count.min(MAX_COUNT) as f32 * 5.0 + 50.0 * recency(now, u.last_used)) as u32
        })
    }

    pub fn item(&self, id: &ItemId) -> Option<Use> {
        self.items.get(id).copied()
    }

    fn apply_pick(&mut self, prefixes: &[String], id: &ItemId, at: i64) {
        let bump = |u: &mut Use| {
            u.count = (u.count + 1).min(MAX_COUNT);
            u.last_used = u.last_used.max(at);
        };
        for p in prefixes {
            bump(
                self.picks
                    .entry(p.clone())
                    .or_default()
                    .entry(id.clone())
                    .or_default(),
            );
        }
        bump(self.items.entry(id.clone()).or_default());
    }
}

enum Cmd {
    Pick { query: String, id: ItemId, at: i64 },
    Reset(mpsc::Sender<Result<()>>),
    Flush(mpsc::Sender<()>),
}

/// The owner of `usage.db`. Cheap to clone; all clones talk to one thread.
#[derive(Clone)]
pub struct UsageStore {
    tx: mpsc::Sender<Cmd>,
    snapshot: Arc<ArcSwap<Usage>>,
}

impl UsageStore {
    /// Opens (or creates) the database and loads it. A file with another
    /// schema version is an error; it is never rewritten.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        init(&conn)?;
        let usage = load(&conn)?;
        let snapshot = Arc::new(ArcSwap::from_pointee(usage));
        let (tx, rx) = mpsc::channel();
        let shared = snapshot.clone();
        thread::Builder::new()
            .name("usage-store".into())
            .spawn(move || run(conn, rx, shared))?;
        Ok(UsageStore { tx, snapshot })
    }

    /// The current usage data. Take it once per keystroke, not per comparison.
    pub fn snapshot(&self) -> Arc<Usage> {
        self.snapshot.load_full()
    }

    /// Records that `id` was picked for `query`. Returns at once; the snapshot
    /// changes when the write has committed.
    pub fn record_pick(&self, query: &str, id: &ItemId, at: i64) {
        let _ = self.tx.send(Cmd::Pick {
            query: query.to_string(),
            id: id.clone(),
            at,
        });
    }

    /// Forgets everything learned.
    pub fn reset(&self) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Cmd::Reset(tx))
            .map_err(|_| anyhow::anyhow!("usage store stopped"))?;
        rx.recv()
            .map_err(|_| anyhow::anyhow!("usage store stopped"))?
    }

    /// Waits until every earlier command has been handled (for tests and
    /// shutdown).
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Cmd::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }
}

fn prefixes(query: &str) -> Vec<String> {
    let folded = text::fold(query.trim());
    let chars: Vec<char> = folded.chars().take(MAX_PREFIX_CHARS).collect();
    (1..=chars.len())
        .map(|n| chars[..n].iter().collect())
        .collect()
}

fn init(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 0 && version != SCHEMA_VERSION {
        bail!("usage.db has schema version {version}, expected {SCHEMA_VERSION}");
    }
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS query_pick (
             query_prefix TEXT NOT NULL,
             item_id TEXT NOT NULL,
             count INTEGER NOT NULL,
             last_used INTEGER NOT NULL,
             PRIMARY KEY (query_prefix, item_id)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS query_pick_last_used ON query_pick (last_used);
         CREATE TABLE IF NOT EXISTS item_use (
             item_id TEXT PRIMARY KEY,
             count INTEGER NOT NULL,
             last_used INTEGER NOT NULL
         ) WITHOUT ROWID;",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn load(conn: &Connection) -> Result<Usage> {
    let mut usage = Usage::default();
    let mut stmt =
        conn.prepare("SELECT query_prefix, item_id, count, last_used FROM query_pick")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (q, id, count, last_used) = row?;
        usage
            .picks
            .entry(q)
            .or_default()
            .insert(ItemId(id), Use { count, last_used });
    }
    let mut stmt = conn.prepare("SELECT item_id, count, last_used FROM item_use")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (id, count, last_used) = row?;
        usage.items.insert(ItemId(id), Use { count, last_used });
    }
    Ok(usage)
}

fn write_pick(conn: &mut Connection, prefixes: &[String], id: &ItemId, at: i64) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut pick = tx.prepare_cached(
            "INSERT INTO query_pick (query_prefix, item_id, count, last_used) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (query_prefix, item_id) DO UPDATE SET
                 count = MIN(count + 1, ?4), last_used = MAX(last_used, excluded.last_used)",
        )?;
        for p in prefixes {
            pick.execute(params![p, id.as_str(), at, MAX_COUNT])?;
        }
        tx.execute(
            "INSERT INTO item_use (item_id, count, last_used) VALUES (?1, 1, ?2)
             ON CONFLICT (item_id) DO UPDATE SET
                 count = MIN(count + 1, ?3), last_used = MAX(last_used, excluded.last_used)",
            params![id.as_str(), at, MAX_COUNT],
        )?;
        // Forget the oldest picks beyond the cap.
        tx.execute(
            "DELETE FROM query_pick WHERE (query_prefix, item_id) IN (
                 SELECT query_prefix, item_id FROM query_pick ORDER BY last_used
                 LIMIT MAX(0, (SELECT COUNT(*) FROM query_pick) - ?1))",
            params![MAX_PICK_ROWS],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn run(mut conn: Connection, rx: mpsc::Receiver<Cmd>, snapshot: Arc<ArcSwap<Usage>>) {
    for cmd in rx {
        match cmd {
            Cmd::Pick { query, id, at } => {
                let prefixes = prefixes(&query);
                match write_pick(&mut conn, &prefixes, &id, at) {
                    Ok(()) => {
                        let mut next = (**snapshot.load()).clone();
                        next.apply_pick(&prefixes, &id, at);
                        snapshot.store(Arc::new(next));
                    }
                    Err(e) => log::warn!("recording a pick failed: {e:#}"),
                }
            }
            Cmd::Reset(reply) => {
                let r = conn
                    .execute_batch("DELETE FROM query_pick; DELETE FROM item_use;")
                    .map_err(anyhow::Error::from)
                    .map(|_| snapshot.store(Arc::new(Usage::default())));
                let _ = reply.send(r);
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

    #[test]
    fn picks_teach_every_prefix_and_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let chrome = ItemId::new("app", "chrome");
        let now = 1_700_000_000_000;
        {
            let store = UsageStore::open(&path).unwrap();
            store.record_pick("Chr", &chrome, now);
            store.record_pick("chr", &chrome, now);
            store.flush();
            let u = store.snapshot();
            assert!(u.query_boost("c", &chrome, now) > 0);
            assert!(u.query_boost("chr", &chrome, now) > 0);
            assert_eq!(u.query_boost("chro", &chrome, now), 0);
            assert_eq!(u.item(&chrome).unwrap().count, 2);
        }
        let store = UsageStore::open(&path).unwrap();
        assert_eq!(store.snapshot().item(&chrome).unwrap().count, 2);
        store.reset().unwrap();
        assert_eq!(store.snapshot().item(&chrome), None);
    }

    #[test]
    fn clock_steps_back_dont_rewind_recency_and_counts_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(&dir.path().join("usage.db")).unwrap();
        let id = ItemId::new("app", "x");
        let now = 1_700_000_000_000;
        store.record_pick("x", &id, now);
        store.record_pick("x", &id, now - 30 * DAY_MS);
        for _ in 0..30 {
            store.record_pick("x", &id, now);
        }
        store.flush();
        let u = store.snapshot().item(&id).unwrap();
        assert_eq!(u.last_used, now);
        assert_eq!(u.count, MAX_COUNT);
    }

    #[test]
    fn other_schema_versions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 7)
            .unwrap();
        assert!(UsageStore::open(&path).is_err());
    }
}
