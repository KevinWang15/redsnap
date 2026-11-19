//! `config.json`: the settings, owned by the native process. Electron edits
//! them through IPC; this module validates, loads and saves.
//!
//! Decoding is strict: unknown fields and duplicate keys are errors. A
//! malformed file is never overwritten silently: it is moved aside, the last
//! good copy (or the defaults) is used, and the problem is reported.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    /// The global shortcut, in `global-hotkey` syntax (`Alt+Space`).
    pub hotkey: String,
    pub start_at_login: bool,
    /// Visible result rows.
    pub rows: u8,
    pub providers: Providers,
    pub web_searches: Vec<WebSearch>,
    pub quick_links: Vec<QuickLink>,
    /// Extra names for apps: item id → aliases.
    pub app_aliases: BTreeMap<String, Vec<String>>,
    /// Item ids of apps left out of search.
    pub hidden_apps: Vec<String>,
    /// Folders scanned for apps besides the OS defaults.
    pub extra_app_folders: Vec<PathBuf>,
    /// Optional in the file: added after the first release of this format.
    #[serde(default)]
    pub clipboard: Clipboard,
}

/// Clipboard history (docs/clipboard.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Clipboard {
    pub enabled: bool,
    /// Opens the window straight in the clipboard view.
    pub hotkey: String,
    pub keep_days: u32,
    pub max_entries: u32,
    /// Copies from these apps are skipped (by executable or app name, e.g.
    /// `KeePassXC.exe`, `1Password`).
    pub excluded_apps: Vec<String>,
    /// List skipped copies (dimmed) with the reason.
    pub show_skipped: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Clipboard {
            enabled: true,
            hotkey: "Ctrl+Alt+V".into(),
            keep_days: 30,
            max_entries: 5000,
            excluded_apps: Vec::new(),
            show_skipped: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Providers {
    pub apps: bool,
    pub calculator: bool,
    pub web_searches: bool,
    pub quick_links: bool,
    pub open: bool,
    pub commands: bool,
    pub system_commands: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WebSearch {
    /// Stable id (item ids and learned ranking use it).
    pub id: String,
    pub name: String,
    /// Typed before the search words: `g rust`.
    pub keyword: String,
    /// `{query}` is replaced by the URL-encoded words.
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QuickLink {
    pub id: String,
    pub name: String,
    /// Optional keyword; with `{query}` in the URL it takes words like a web
    /// search.
    pub keyword: String,
    pub url: String,
}

impl Default for Config {
    fn default() -> Self {
        let search = |id: &str, name: &str, keyword: &str, url: &str| WebSearch {
            id: id.into(),
            name: name.into(),
            keyword: keyword.into(),
            url: url.into(),
        };
        Config {
            version: VERSION,
            // Option+Space on macOS (Alt is Option there).
            hotkey: "Alt+Space".into(),
            start_at_login: false,
            rows: 5,
            providers: Providers {
                apps: true,
                calculator: true,
                web_searches: true,
                quick_links: true,
                open: true,
                commands: true,
                system_commands: true,
            },
            web_searches: vec![
                search("google", "Google", "g", "https://www.google.com/search?q={query}"),
                search("bing", "Bing", "b", "https://www.bing.com/search?q={query}"),
                search("baidu", "Baidu", "bd", "https://www.baidu.com/s?wd={query}"),
                search("github", "GitHub", "gh", "https://github.com/search?q={query}"),
            ],
            quick_links: Vec::new(),
            app_aliases: BTreeMap::new(),
            hidden_apps: Vec::new(),
            extra_app_folders: Vec::new(),
            clipboard: Clipboard::default(),
        }
    }
}

impl Config {
    /// Checks everything a user (or the settings UI) could get wrong. Returns
    /// the first problem, naming the field.
    pub fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            bail!("version: expected {VERSION}, found {}", self.version);
        }
        if self.hotkey.trim().is_empty() {
            bail!("hotkey: empty");
        }
        if !(1..=12).contains(&self.rows) {
            bail!("rows: must be 1 to 12, found {}", self.rows);
        }
        let mut ids = HashSet::new();
        let mut keywords = HashSet::new();
        for (field, id, keyword, url) in self
            .web_searches
            .iter()
            .map(|w| ("webSearches", &w.id, &w.keyword, &w.url))
            .chain(self.quick_links.iter().map(|l| ("quickLinks", &l.id, &l.keyword, &l.url)))
        {
            if id.trim().is_empty() || !ids.insert(id.as_str()) {
                bail!("{field}: id {id:?} is empty or used twice");
            }
            if keyword.contains(char::is_whitespace) {
                bail!("{field}: keyword {keyword:?} contains a space");
            }
            if !keyword.is_empty() && !keywords.insert(keyword.to_lowercase()) {
                bail!("{field}: keyword {keyword:?} is used twice");
            }
            if !crate::item::url_allowed(url) {
                bail!("{field}: {url:?} isn't an http(s) URL");
            }
        }
        if !(1..=3650).contains(&self.clipboard.keep_days) {
            bail!("clipboard.keepDays: must be 1 to 3650, found {}", self.clipboard.keep_days);
        }
        if !(10..=100_000).contains(&self.clipboard.max_entries) {
            bail!("clipboard.maxEntries: must be 10 to 100000, found {}", self.clipboard.max_entries);
        }
        if self.web_searches.iter().any(|w| !w.url.contains("{query}")) {
            bail!("webSearches: every URL needs {{query}}");
        }
        Ok(())
    }

    /// Parses `config.json` text strictly and validates it.
    pub fn from_json(text: &str) -> Result<Self> {
        reject_duplicate_keys(text)?;
        let config: Config = serde_json::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("config serializes") + "\n"
    }
}

/// What happened when loading.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Set when `config.json` was unreadable: what was wrong, and where the
    /// bad file was moved.
    pub problem: Option<String>,
}

/// Loads `<dir>/config.json`. A missing file gives the defaults (and writes
/// them). A malformed one is renamed to `config.invalid-<time>.json`, then
/// `config.json.bak` or the defaults are used, and `problem` says so.
pub fn load(dir: &Path) -> Result<Loaded> {
    let path = dir.join("config.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let config = Config::default();
            save(dir, &config)?;
            return Ok(Loaded { config, problem: None });
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    match Config::from_json(&text) {
        Ok(config) => Ok(Loaded { config, problem: None }),
        Err(error) => {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let aside = dir.join(format!("config.invalid-{stamp}.json"));
            fs::rename(&path, &aside).with_context(|| format!("moving {} aside", path.display()))?;
            let backup = fs::read_to_string(dir.join("config.json.bak")).ok().and_then(|t| Config::from_json(&t).ok());
            let used = if backup.is_some() { "the last good settings" } else { "the default settings" };
            let config = backup.unwrap_or_default();
            save(dir, &config)?;
            Ok(Loaded {
                config,
                problem: Some(format!(
                    "config.json couldn't be read ({error:#}); it was moved to {} and {used} are in use",
                    aside.display()
                )),
            })
        }
    }
}

/// Saves atomically: the current file becomes `config.json.bak`, the new one
/// is written to a temporary file in the same folder, flushed, and renamed
/// over `config.json`.
pub fn save(dir: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    fs::create_dir_all(dir)?;
    let path = dir.join("config.json");
    if path.exists() {
        let _ = fs::copy(&path, dir.join("config.json.bak"));
    }
    let tmp = dir.join(format!(".config.json.{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(config.to_json().as_bytes())?;
        f.sync_all()?;
    }
    rename_with_retry(&tmp, &path)
}

/// Antivirus and indexers briefly lock files on Windows; retry the rename
/// for up to 5 s before giving up.
fn rename_with_retry(from: &Path, to: &Path) -> Result<()> {
    let mut delay = Duration::from_millis(5);
    let mut waited = Duration::ZERO;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && waited < Duration::from_secs(5) => {
                std::thread::sleep(delay);
                waited += delay;
                delay = (delay * 2).min(Duration::from_millis(200));
            }
            Err(e) => {
                let _ = fs::remove_file(from);
                return Err(e).with_context(|| format!("replacing {}", to.display()));
            }
        }
    }
}

/// serde_json keeps the last of duplicate keys silently; refuse them.
fn reject_duplicate_keys(text: &str) -> Result<()> {
    struct NoDup;
    impl<'de> Deserialize<'de> for NoDup {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(NoDupVisitor)
        }
    }
    struct NoDupVisitor;
    impl<'de> Visitor<'de> for NoDupVisitor {
        type Value = NoDup;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("JSON")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<NoDup, A::Error> {
            let mut seen = HashSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key.clone()) {
                    return Err(de::Error::custom(format!("duplicate key {key:?}")));
                }
                map.next_value::<NoDup>()?;
            }
            Ok(NoDup)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<NoDup, A::Error> {
            while seq.next_element::<NoDup>()?.is_some() {}
            Ok(NoDup)
        }
        fn visit_bool<E>(self, _: bool) -> Result<NoDup, E> {
            Ok(NoDup)
        }
        fn visit_i64<E>(self, _: i64) -> Result<NoDup, E> {
            Ok(NoDup)
        }
        fn visit_u64<E>(self, _: u64) -> Result<NoDup, E> {
            Ok(NoDup)
        }
        fn visit_f64<E>(self, _: f64) -> Result<NoDup, E> {
            Ok(NoDup)
        }
        fn visit_str<E>(self, _: &str) -> Result<NoDup, E> {
            Ok(NoDup)
        }
        fn visit_unit<E>(self) -> Result<NoDup, E> {
            Ok(NoDup)
        }
    }
    serde_json::from_str::<NoDup>(text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_validate() {
        let c = Config::default();
        c.validate().unwrap();
        assert_eq!(Config::from_json(&c.to_json()).unwrap(), c);
    }

    #[test]
    fn strict_decoding() {
        let good = Config::default().to_json();
        let unknown = good.replacen("\"rows\"", "\"colour\": 1,\n  \"rows\"", 1);
        assert!(Config::from_json(&unknown).is_err());
        let dup = good.replacen("\"rows\": 5", "\"rows\": 5,\n  \"rows\": 6", 1);
        let err = Config::from_json(&dup).unwrap_err().to_string();
        assert!(err.contains("duplicate key"), "{err}");
    }

    #[test]
    fn validation_names_the_field() {
        let mut c = Config::default();
        c.web_searches[1].keyword = "g".into();
        assert!(c.validate().unwrap_err().to_string().contains("used twice"));
        let mut c = Config::default();
        c.quick_links.push(QuickLink { id: "x".into(), name: "x".into(), keyword: "".into(), url: "file:///etc".into() });
        assert!(c.validate().unwrap_err().to_string().starts_with("quickLinks"));
    }

    #[test]
    fn malformed_file_is_moved_aside_and_backup_used() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config { rows: 7, ..Default::default() };
        save(dir.path(), &c).unwrap();
        save(dir.path(), &c).unwrap(); // makes config.json.bak
        fs::write(dir.path().join("config.json"), "{ not json").unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.config.rows, 7);
        let problem = loaded.problem.unwrap();
        assert!(problem.contains("last good settings"), "{problem}");
        let moved = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("config.invalid-"));
        assert!(moved);
    }

    #[test]
    fn files_without_the_clipboard_section_still_load() {
        let mut v: serde_json::Value = serde_json::from_str(&Config::default().to_json()).unwrap();
        v.as_object_mut().unwrap().remove("clipboard");
        let c = Config::from_json(&v.to_string()).unwrap();
        assert_eq!(c.clipboard, Clipboard::default());
    }

    #[test]
    fn missing_file_writes_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(dir.path()).unwrap();
        assert!(loaded.problem.is_none());
        assert!(dir.path().join("config.json").exists());
    }
}
