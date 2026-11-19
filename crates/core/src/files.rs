//! Files: searching the OS's file index (`f report`) and browsing folders by
//! typing a path (`~/dev/`, `C:\Users\`). docs/files.md has the behavior.
//!
//! The OS side (Everything, Spotlight, plocate, reading folders) runs on a
//! background thread in `redsnap_platform::files`. This module decides what a
//! query asks for ([`Files::request`]), keeps the latest answer
//! ([`Files::set_results`]), and turns it into results on every keystroke
//! ([`FilesProvider`]) without doing any IO itself.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::item::{Action, IconRef, Item, ItemId, ProviderId};
use crate::matcher::{Match, MatchClass, Names, PreparedQuery};
use crate::query::{Query, Target};
use crate::search::{Candidate, Provider, SearchContext};

/// A file or folder from the index or a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    /// The last component (a drive's own name for a drive).
    pub name: String,
    pub is_dir: bool,
    /// A dotfile, or hidden by the OS.
    pub hidden: bool,
}

/// What the OS side is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRequest {
    /// The file index, for these words. Empty words only check that the
    /// index can be used.
    Search(String),
    /// A folder's entries. The typed name after the folder filters them
    /// here, so typing within a folder needs no new listing.
    List(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hits(Vec<FileHit>),
    /// The index can't be used: why, what to do, and where to read more.
    Unavailable {
        title: String,
        subtitle: String,
        url: Option<String>,
    },
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileResults {
    pub request: FileRequest,
    pub outcome: Outcome,
}

/// What the query is for, shared by the launcher (which sends requests and
/// receives results) and the provider (which shows them).
pub struct Files {
    /// `f …` searches the index.
    search: bool,
    /// Typed paths list folders.
    browse: bool,
    /// The index's name for the hint row ("Everything", "Spotlight", …).
    backend: &'static str,
    /// A line on what the index understands, for the hint row.
    syntax: &'static str,
    home: Option<PathBuf>,
    latest: Mutex<Option<Arc<Latest>>>,
}

/// The newest results, with the names prepared once for matching.
struct Latest {
    request: FileRequest,
    outcome: Outcome,
    names: Vec<Names>,
}

/// What a typed path means: the folder (as typed and resolved) and the start
/// of a name in it.
#[derive(Debug, PartialEq, Eq)]
struct TypedPath<'a> {
    /// Up to and including the last separator: `~/dev/`.
    typed_dir: &'a str,
    dir: PathBuf,
    filter: &'a str,
    /// The separator to add after a folder's name when browsing into it.
    sep: char,
}

/// Whether the text is a path rather than a search: absolute, `~`, a drive
/// or a UNC share.
pub fn looks_like_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with("~/")
        || s == "~"
        || s.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/'))
}

/// `C:\…` and `\\server\…` take both separators; other paths only `/`.
fn windows_style(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with("\\\\") || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

fn typed_path<'a>(text: &'a str, home: Option<&Path>) -> Option<TypedPath<'a>> {
    if !looks_like_path(text) {
        return None;
    }
    if text == "~" {
        return Some(TypedPath {
            typed_dir: "~",
            dir: home?.to_path_buf(),
            filter: "",
            sep: '/',
        });
    }
    let win = windows_style(text);
    let cut = text.rfind(|c| c == '/' || (win && c == '\\'))?;
    let (typed_dir, filter) = (&text[..=cut], &text[cut + 1..]);
    let sep = typed_dir.chars().last().unwrap_or('/');
    let dir = match typed_dir.strip_prefix('~') {
        Some(rest) => home?.join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(typed_dir),
    };
    Some(TypedPath {
        typed_dir,
        dir,
        filter,
        sep,
    })
}

/// `~/dev` → the home folder joined with `dev`.
fn expand(text: &str, home: Option<&Path>) -> PathBuf {
    match (text.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => text.into(),
    }
}

impl Files {
    pub fn new(
        search: bool,
        browse: bool,
        backend: &'static str,
        syntax: &'static str,
        home: Option<PathBuf>,
    ) -> Self {
        Files {
            search,
            browse,
            backend,
            syntax,
            home,
            latest: Mutex::new(None),
        }
    }

    /// What `q` needs from the OS side, if anything.
    pub fn request(&self, q: &Query) -> Option<FileRequest> {
        match &q.route {
            Some((Target::Files, rest)) if self.search => Some(FileRequest::Search(rest.clone())),
            Some(_) => None,
            None if self.browse => {
                typed_path(&q.text, self.home.as_deref()).map(|t| FileRequest::List(t.dir))
            }
            None => None,
        }
    }

    pub fn set_results(&self, r: FileResults) {
        let names = match &r.outcome {
            Outcome::Hits(hits) => hits.iter().map(|h| Names::new(&h.name, [])).collect(),
            _ => Vec::new(),
        };
        *self.latest.lock().unwrap() = Some(Arc::new(Latest {
            request: r.request,
            outcome: r.outcome,
            names,
        }));
    }

    fn latest(&self) -> Option<Arc<Latest>> {
        self.latest.lock().unwrap().clone()
    }
}

/// The results for [`Files`]: index hits under `f`, folder entries for a
/// typed path.
pub struct FilesProvider(pub Arc<Files>);

fn info(key: &str, title: String, subtitle: String, primary: Action) -> Item {
    Item {
        id: ItemId::new("files", key),
        provider: ProviderId::Files,
        title,
        subtitle,
        icon: IconRef::Builtin("info"),
        primary,
        secondary: None,
        learnable: false,
    }
}

fn parent_text(path: &Path) -> String {
    path.parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Keeps the index's or the listing's order among equal matches.
fn in_order(i: usize) -> u32 {
    u32::MAX / 2 - i as u32
}

impl FilesProvider {
    fn index(&self, words: &str, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        let files = &self.0;
        let latest = files.latest();
        let current = latest
            .as_ref()
            .filter(|l| matches!(&l.request, FileRequest::Search(w) if w == words));
        match latest.as_deref().map(|l| (&l.request, &l.outcome)) {
            Some((
                FileRequest::Search(_),
                Outcome::Unavailable {
                    title,
                    subtitle,
                    url,
                },
            )) => {
                let action = url.clone().map_or(Action::Nothing, Action::OpenUrl);
                out.push(Candidate::top(info(
                    "unavailable",
                    title.clone(),
                    subtitle.clone(),
                    action,
                )));
                return;
            }
            Some((FileRequest::Search(_), Outcome::Failed(e))) if current.is_some() => {
                out.push(Candidate::top(info(
                    "failed",
                    format!("{} couldn't search", files.backend),
                    e.clone(),
                    Action::Nothing,
                )));
                return;
            }
            _ => {}
        }
        if words.trim().is_empty() {
            let title = format!("Search files with {}", files.backend);
            out.push(Candidate::top(info(
                "hint",
                title,
                files.syntax.to_string(),
                Action::Nothing,
            )));
            return;
        }
        // The answer for these words, or the last one while it's on its way
        // (narrowed to what still matches, so the list doesn't blink).
        let Some(l) = latest
            .as_ref()
            .filter(|l| matches!(l.request, FileRequest::Search(_)))
        else {
            return;
        };
        let Outcome::Hits(hits) = &l.outcome else {
            return;
        };
        let prepared = PreparedQuery::new(words);
        for (i, (hit, names)) in hits.iter().zip(&l.names).enumerate() {
            // The index understands more than names (wildcards, `ext:`,
            // folders): what it found for these words stays, after the
            // name matches.
            let m = match prepared
                .best(names, &mut cx.matcher)
                .filter(|m| m.class != MatchClass::Fuzzy)
            {
                Some(m) => Match {
                    class: m.class,
                    score: in_order(i),
                },
                None if current.is_some() => Match {
                    class: MatchClass::Fuzzy,
                    score: in_order(i),
                },
                None => continue,
            };
            let item = Item {
                id: ItemId::new("file", &hit.path.to_string_lossy()),
                provider: ProviderId::Files,
                title: hit.name.clone(),
                subtitle: parent_text(&hit.path),
                icon: IconRef::Builtin(if hit.is_dir { "folder" } else { "file" }),
                primary: Action::OpenPath(hit.path.clone()),
                secondary: Some(Action::RevealPath(hit.path.clone())),
                learnable: true,
            };
            out.push(Candidate::normal(Arc::new(item), m));
        }
    }

    fn browse(&self, text: &str, t: TypedPath, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        let latest = self.0.latest();
        let listing = latest
            .as_ref()
            .filter(|l| l.request == FileRequest::List(t.dir.clone()));
        let open = |key: &str, title: &str, path: PathBuf| {
            Candidate::top(Item {
                id: ItemId::new("open", key),
                provider: ProviderId::Files,
                title: format!("Open {title}"),
                subtitle: path.display().to_string(),
                icon: IconRef::Builtin("folder"),
                primary: Action::OpenPath(path.clone()),
                secondary: Some(Action::RevealPath(path)),
                learnable: false,
            })
        };
        let Some((Outcome::Hits(hits), names)) = listing.map(|l| (&l.outcome, &l.names)) else {
            // Not listed yet, or the folder can't be read: the OS opens
            // what was typed (and says what's wrong with it).
            out.push(open("path", text, expand(text, self.0.home.as_deref())));
            return;
        };
        if t.filter.is_empty() {
            out.push(open("folder", t.typed_dir, t.dir.clone()));
        }
        let prepared = PreparedQuery::new(t.filter);
        // Folders first, then by name, when nothing is typed after the folder.
        let mut order: Vec<usize> = (0..hits.len()).collect();
        order.sort_by(|&a, &b| {
            hits[b].is_dir.cmp(&hits[a].is_dir).then_with(|| {
                hits[a]
                    .name
                    .to_lowercase()
                    .cmp(&hits[b].name.to_lowercase())
            })
        });
        for (rank, i) in order.into_iter().enumerate() {
            let hit = &hits[i];
            let m = if prepared.is_empty() {
                if hit.hidden {
                    continue;
                }
                Match {
                    class: MatchClass::Exact,
                    score: in_order(rank),
                }
            } else {
                match prepared.best(&names[i], &mut cx.matcher) {
                    // Hidden entries need the start of their name: `.c` finds `.cache`.
                    Some(m) if hit.hidden && m.class > MatchClass::Prefix => continue,
                    Some(m) => m,
                    None => continue,
                }
            };
            let (primary, secondary) = if hit.is_dir {
                (
                    Action::Browse(format!("{}{}{}", t.typed_dir, hit.name, t.sep)),
                    Action::OpenPath(hit.path.clone()),
                )
            } else {
                (
                    Action::OpenPath(hit.path.clone()),
                    Action::RevealPath(hit.path.clone()),
                )
            };
            let item = Item {
                id: ItemId::new("file", &hit.path.to_string_lossy()),
                provider: ProviderId::Files,
                title: hit.name.clone(),
                subtitle: hit.path.display().to_string(),
                icon: IconRef::Builtin(if hit.is_dir { "folder" } else { "file" }),
                primary,
                secondary: Some(secondary),
                learnable: false,
            };
            out.push(Candidate::normal(Arc::new(item), m));
        }
    }
}

impl Provider for FilesProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Files
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        match &q.route {
            Some((Target::Files, rest)) if self.0.search => self.index(rest, cx, out),
            Some(_) => {}
            None if self.0.browse => {
                if let Some(t) = typed_path(&q.text, self.0.home.as_deref()) {
                    self.browse(&q.text, t, cx, out);
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::new_matcher;
    use crate::query::Keywords;

    fn hit(path: &str, is_dir: bool) -> FileHit {
        let name = path.rsplit(['/', '\\']).next().unwrap().to_string();
        FileHit {
            path: path.into(),
            hidden: name.starts_with('.'),
            name,
            is_dir,
        }
    }

    fn files() -> Arc<Files> {
        Arc::new(Files::new(
            true,
            true,
            "Index",
            "Type a name",
            Some("/home/me".into()),
        ))
    }

    fn keywords() -> Keywords {
        Keywords::new([("f".to_string(), Target::Files)])
    }

    fn run(files: &Arc<Files>, text: &str) -> Vec<Candidate> {
        let q = Query::parse(text, &keywords());
        let mut cx = SearchContext {
            matcher: new_matcher(),
            now: 0,
        };
        let mut out = Vec::new();
        FilesProvider(files.clone()).search(&q, &mut cx, &mut out);
        out.sort_by(|a, b| a.slot.cmp(&b.slot).then(a.m.better(&b.m)));
        out
    }

    fn titles(c: &[Candidate]) -> Vec<&str> {
        c.iter().map(|c| c.item.title.as_str()).collect()
    }

    #[test]
    fn typed_paths() {
        let home = Path::new("/home/me");
        let t = typed_path("~/dev/re", Some(home)).unwrap();
        assert_eq!(
            (t.typed_dir, t.dir.as_path(), t.filter, t.sep),
            ("~/dev/", Path::new("/home/me/dev"), "re", '/')
        );
        let t = typed_path("~", Some(home)).unwrap();
        assert_eq!((t.dir.as_path(), t.filter), (home, ""));
        let t = typed_path("C:\\Users\\ke", None).unwrap();
        assert_eq!((t.typed_dir, t.filter, t.sep), ("C:\\Users\\", "ke", '\\'));
        let t = typed_path("C:/Program Files/Mi", None).unwrap();
        assert_eq!(
            (t.typed_dir, t.filter, t.sep),
            ("C:/Program Files/", "Mi", '/')
        );
        // Backslashes are part of a name on Unix-style paths.
        let t = typed_path("/tmp/a\\b", None).unwrap();
        assert_eq!((t.typed_dir, t.filter), ("/tmp/", "a\\b"));
        assert!(typed_path("report", None).is_none());
    }

    #[test]
    fn requests() {
        let f = files();
        let req = |text: &str| f.request(&Query::parse(text, &keywords()));
        assert_eq!(
            req("f report q3"),
            Some(FileRequest::Search("report q3".into()))
        );
        assert_eq!(req("f "), Some(FileRequest::Search(String::new())));
        assert_eq!(
            req("~/dev/re"),
            Some(FileRequest::List("/home/me/dev".into()))
        );
        assert_eq!(req("report"), None);
        let off = Files::new(false, false, "Index", "", None);
        assert_eq!(off.request(&Query::parse("f x", &keywords())), None);
        assert_eq!(off.request(&Query::parse("/tmp/", &keywords())), None);
    }

    #[test]
    fn browsing_a_folder() {
        let f = files();
        // Before the listing arrives, the typed path itself.
        let c = run(&f, "~/dev/re");
        assert_eq!(
            c[0].item.primary,
            Action::OpenPath("/home/me/dev/re".into())
        );
        f.set_results(FileResults {
            request: FileRequest::List("/home/me/dev".into()),
            outcome: Outcome::Hits(vec![
                hit("/home/me/dev/notes.txt", false),
                hit("/home/me/dev/redsnap", true),
                hit("/home/me/dev/.config", true),
                hit("/home/me/dev/apps", true),
            ]),
        });
        // Nothing typed after the folder: open it, then folders, then files; no hidden ones.
        assert_eq!(
            titles(&run(&f, "~/dev/")),
            ["Open ~/dev/", "apps", "redsnap", "notes.txt"]
        );
        // A folder browses into it; a file opens.
        let c = run(&f, "~/dev/re");
        assert_eq!(titles(&c), ["redsnap"]);
        assert_eq!(c[0].item.primary, Action::Browse("~/dev/redsnap/".into()));
        assert_eq!(
            c[0].item.secondary,
            Some(Action::OpenPath("/home/me/dev/redsnap".into()))
        );
        assert_eq!(
            run(&f, "~/dev/no")[0].item.primary,
            Action::OpenPath("/home/me/dev/notes.txt".into())
        );
        // Hidden entries by the start of their name.
        assert_eq!(titles(&run(&f, "~/dev/.c")), [".config"]);
        assert!(titles(&run(&f, "~/dev/fig")).is_empty());
    }

    #[test]
    fn searching_the_index() {
        let f = files();
        assert_eq!(titles(&run(&f, "f ")), ["Search files with Index"]);
        assert_eq!(run(&f, "f ")[0].item.primary, Action::Nothing);
        f.set_results(FileResults {
            request: FileRequest::Search("rep".into()),
            outcome: Outcome::Hits(vec![
                hit("/a/old-report.txt", false),
                hit("/b/report.pdf", false),
                hit("/c/q3", true), // the index matched it some other way
            ]),
        });
        let c = run(&f, "f rep");
        assert_eq!(titles(&c), ["report.pdf", "old-report.txt", "q3"]);
        assert_eq!(c[0].item.subtitle, "/b");
        assert_eq!(
            c[0].item.secondary,
            Some(Action::RevealPath("/b/report.pdf".into()))
        );
        // While the answer for "repo" is on its way: the old one, narrowed.
        assert_eq!(titles(&run(&f, "f repo")), ["report.pdf", "old-report.txt"]);
        f.set_results(FileResults {
            request: FileRequest::Search(String::new()),
            outcome: Outcome::Unavailable {
                title: "Index isn't running".into(),
                subtitle: "Start it".into(),
                url: Some("https://example.com".into()),
            },
        });
        let c = run(&f, "f rep");
        assert_eq!(titles(&c), ["Index isn't running"]);
        assert_eq!(
            c[0].item.primary,
            Action::OpenUrl("https://example.com".into())
        );
    }
}
