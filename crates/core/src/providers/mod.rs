//! The phase-1 providers. All of them work on in-memory data.

pub mod apps;

use std::sync::Arc;

use crate::calc;
use crate::config::{QuickLink, Snippet, WebSearch};
use crate::item::{
    Action, IconRef, InternalCommand, Item, ItemId, ProviderId, SystemCommand, url_allowed,
};
use crate::matcher::{MatchClass, Names};
use crate::query::{Query, Target};
use crate::search::{Candidate, Provider, SearchContext};

// ----- calculator ------------------------------------------------------------

pub struct CalculatorProvider;

impl Provider for CalculatorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Calculator
    }

    fn search(&self, q: &Query, _: &mut SearchContext, out: &mut Vec<Candidate>) {
        if q.route.is_some() {
            return;
        }
        if let Some(v) = calc::evaluate(&q.text) {
            let result = calc::format(v);
            out.push(Candidate::top(Item {
                id: ItemId::new("calc", "result"),
                provider: ProviderId::Calculator,
                title: result.clone(),
                subtitle: format!(
                    "{} = {result} · Enter to copy",
                    q.text.trim_end_matches('=').trim()
                ),
                icon: IconRef::Builtin("calculator"),
                primary: Action::CopyText(result),
                secondary: None,
                learnable: false,
            }));
        }
    }
}

// ----- open a typed URL -------------------------------------------------------
// (Typed paths are folder browsing: crate::files.)

pub struct OpenProvider;

fn looks_like_url(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if s.contains(char::is_whitespace) {
        return None;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(s.to_string());
    }
    // `www.example.com`, `example.com/path`: a dot in the host and a common
    // TLD. File names like `config.json` must not turn into URLs, so the TLD
    // comes from a short list rather than "any letters".
    const TLDS: &[&str] = &[
        "com", "net", "org", "io", "dev", "app", "ai", "co", "cn", "me", "so", "xyz", "info",
        "edu", "gov", "uk", "de", "jp", "fr", "hk", "tw", "us", "cc", "tv",
    ];
    let host = lower.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    let tld = host.rsplit('.').next().unwrap_or("");
    let plausible = host.contains('.')
        && TLDS.contains(&tld)
        && host.split('.').all(|label| !label.is_empty())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    (lower.starts_with("www.") || plausible).then(|| format!("https://{s}"))
}

impl Provider for OpenProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Open
    }

    fn search(&self, q: &Query, _: &mut SearchContext, out: &mut Vec<Candidate>) {
        if q.route.is_some() || q.is_empty() || crate::files::looks_like_path(&q.text) {
            return;
        }
        if let Some(url) = looks_like_url(&q.text).filter(|u| url_allowed(u)) {
            let text = q.text.as_str();
            out.push(Candidate::top(Item {
                id: ItemId::new("open", "url"),
                provider: ProviderId::Open,
                title: format!("Open {text}"),
                subtitle: url.clone(),
                icon: IconRef::Builtin("link"),
                primary: Action::OpenUrl(url.clone()),
                secondary: Some(Action::CopyText(url)),
                learnable: false,
            }));
        }
    }
}

// ----- RedSnap and system commands ---------------------------------------------

struct Command {
    item: Arc<Item>,
    names: Names,
}

pub struct CommandsProvider {
    commands: Vec<Command>,
}

impl CommandsProvider {
    /// `system`: which OS commands this platform supports (and the user has
    /// enabled). RedSnap's own commands are always there.
    pub fn new(system: &[SystemCommand]) -> Self {
        let mut commands = Vec::new();
        let mut add = |key: &str,
                       title: &'static str,
                       subtitle: &'static str,
                       extra: &[&str],
                       action,
                       icon| {
            let item = Item {
                id: ItemId::new("cmd", key),
                provider: ProviderId::Commands,
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                icon: IconRef::Builtin(icon),
                primary: action,
                secondary: None,
                learnable: true,
            };
            commands.push(Command {
                item: Arc::new(item),
                names: Names::new(title, extra.iter().copied()),
            });
        };
        use InternalCommand::*;
        add(
            "settings",
            "RedSnap Settings",
            "Open RedSnap's settings",
            &["preferences", "config"],
            Action::RedSnap(OpenSettings),
            "gear",
        );
        add(
            "reload-apps",
            "Reload Apps",
            "Scan for installed apps again",
            &["rescan", "refresh"],
            Action::RedSnap(ReloadApps),
            "refresh",
        );
        add(
            "about",
            "About RedSnap",
            "Version and data folders",
            &[],
            Action::RedSnap(About),
            "info",
        );
        add(
            "quit",
            "Quit RedSnap",
            "Stop RedSnap until it's started again",
            &["exit"],
            Action::RedSnap(Quit),
            "power",
        );
        for &c in system {
            let (key, title, subtitle, extra): (&str, &'static str, &'static str, &[&str]) = match c
            {
                SystemCommand::Lock => ("lock", "Lock Screen", "Lock this computer", &["lock"]),
                SystemCommand::Sleep => {
                    ("sleep", "Sleep", "Put this computer to sleep", &["suspend"])
                }
                SystemCommand::LogOut => (
                    "log-out",
                    "Log Out",
                    "End this session (asks first)",
                    &["sign out", "logout"],
                ),
                SystemCommand::Restart => (
                    "restart",
                    "Restart",
                    "Restart this computer (asks first)",
                    &["reboot"],
                ),
                SystemCommand::ShutDown => (
                    "shut-down",
                    "Shut Down",
                    "Turn this computer off (asks first)",
                    &["power off", "shutdown"],
                ),
                SystemCommand::EmptyTrash => (
                    "empty-trash",
                    "Empty Trash",
                    "Delete everything in the Trash",
                    &["recycle bin"],
                ),
            };
            add(key, title, subtitle, extra, Action::System(c), "power");
        }
        CommandsProvider { commands }
    }
}

impl Provider for CommandsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Commands
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        if q.route.is_some() || q.is_empty() {
            return;
        }
        for c in &self.commands {
            // Commands need a literal match: "lock" shouldn't come up for "lo ck"-ish typos.
            let Some(m) = q
                .prepared
                .best(&c.names, &mut cx.matcher)
                .filter(|m| m.class != MatchClass::Fuzzy)
            else {
                continue;
            };
            out.push(Candidate::normal(c.item.clone(), m));
        }
    }
}

// ----- snippets ---------------------------------------------------------------

/// Snippets found by name, trigger or text; Enter types one into the app
/// that was in front.
pub struct SnippetsProvider {
    snippets: Vec<(Arc<Item>, Names)>,
}

impl SnippetsProvider {
    pub fn new(snippets: &[Snippet]) -> Self {
        let snippets = snippets
            .iter()
            .map(|s| {
                let preview: String = s
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect();
                let item = Item {
                    id: ItemId::new("snippet", &s.id),
                    provider: ProviderId::Snippets,
                    title: if s.name.is_empty() {
                        s.trigger.clone()
                    } else {
                        s.name.clone()
                    },
                    subtitle: if s.trigger.is_empty() {
                        preview
                    } else {
                        format!("{} · {preview}", s.trigger)
                    },
                    icon: IconRef::Builtin("snippet"),
                    primary: Action::Snippet(s.id.clone()),
                    secondary: None,
                    learnable: true,
                };
                let names = Names::new(
                    &item.title,
                    [s.trigger.as_str()].into_iter().filter(|t| !t.is_empty()),
                );
                (Arc::new(item), names)
            })
            .collect();
        SnippetsProvider { snippets }
    }
}

impl Provider for SnippetsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Snippets
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        // `sn` + space routes here: every snippet, or those matching the rest
        // (fuzzy allowed). Ordinary queries find snippets by literal matches.
        let (only, text) = match &q.route {
            Some((Target::Snippets, rest)) => (true, rest.as_str()),
            Some(_) => return,
            None if q.is_empty() => return,
            None => (false, q.text.as_str()),
        };
        let prepared = crate::matcher::PreparedQuery::new(text);
        for (item, names) in &self.snippets {
            if only && prepared.is_empty() {
                out.push(Candidate::normal(
                    item.clone(),
                    crate::matcher::Match {
                        class: MatchClass::Exact,
                        score: 0,
                    },
                ));
            } else if let Some(m) = prepared
                .best(names, &mut cx.matcher)
                .filter(|m| only || m.class != MatchClass::Fuzzy)
            {
                out.push(Candidate::normal(item.clone(), m));
            }
        }
    }
}

// ----- web searches and quick links ----------------------------------------------

/// `{query}` replaced by `words`, URL-encoded.
pub fn fill_template(url: &str, words: &str) -> String {
    url.replace("{query}", &percent_encode(words))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub struct WebSearchProvider {
    searches: Vec<(WebSearch, Names)>,
}

impl WebSearchProvider {
    pub fn new(searches: Vec<WebSearch>) -> Self {
        let searches = searches
            .into_iter()
            .map(|w| {
                let names = Names::new(&w.name, [w.keyword.as_str()]);
                (w, names)
            })
            .collect();
        WebSearchProvider { searches }
    }

    fn item(w: &WebSearch, words: &str) -> Item {
        let url = fill_template(&w.url, words);
        Item {
            id: ItemId::new("web", &w.id),
            provider: ProviderId::WebSearch,
            title: if words.is_empty() {
                format!("Search {}", w.name)
            } else {
                format!("Search {} for “{words}”", w.name)
            },
            subtitle: url.clone(),
            icon: IconRef::Builtin("search"),
            primary: Action::OpenUrl(url.clone()),
            secondary: Some(Action::CopyText(url)),
            learnable: true,
        }
    }
}

impl Provider for WebSearchProvider {
    fn id(&self) -> ProviderId {
        ProviderId::WebSearch
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        match &q.route {
            Some((Target::WebSearch(id), rest)) => {
                if let Some((w, _)) = self.searches.iter().find(|(w, _)| &w.id == id) {
                    out.push(Candidate::top(Self::item(w, rest)));
                }
            }
            Some(_) => {}
            None if q.is_empty() => {}
            None => {
                // "google" finds the Google search itself, waiting for words.
                for (w, names) in &self.searches {
                    if let Some(m) = q
                        .prepared
                        .best(names, &mut cx.matcher)
                        .filter(|m| m.class <= MatchClass::WordPrefix)
                    {
                        out.push(Candidate::normal(Arc::new(Self::item(w, "")), m));
                    }
                }
                // Nothing else matched: offer the first search for the whole text.
                if let Some((w, _)) = self.searches.first() {
                    let mut fallback = Self::item(w, &q.text);
                    fallback.id = ItemId::new("web-fallback", &w.id);
                    fallback.learnable = false;
                    out.push(Candidate::fallback(fallback));
                }
            }
        }
    }
}

pub struct QuickLinksProvider {
    links: Vec<(QuickLink, Names)>,
}

impl QuickLinksProvider {
    pub fn new(links: Vec<QuickLink>) -> Self {
        let links = links
            .into_iter()
            .map(|l| {
                let names = Names::new(
                    &l.name,
                    [l.keyword.as_str()].into_iter().filter(|k| !k.is_empty()),
                );
                (l, names)
            })
            .collect();
        QuickLinksProvider { links }
    }

    fn item(l: &QuickLink, words: &str) -> Item {
        let url = fill_template(&l.url, words);
        Item {
            id: ItemId::new("link", &l.id),
            provider: ProviderId::QuickLinks,
            title: l.name.clone(),
            subtitle: url.clone(),
            icon: IconRef::Builtin("link"),
            primary: Action::OpenUrl(url.clone()),
            secondary: Some(Action::CopyText(url)),
            learnable: true,
        }
    }
}

impl Provider for QuickLinksProvider {
    fn id(&self) -> ProviderId {
        ProviderId::QuickLinks
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        match &q.route {
            Some((Target::QuickLink(id), rest)) => {
                if let Some((l, _)) = self.links.iter().find(|(l, _)| &l.id == id) {
                    out.push(Candidate::top(Self::item(l, rest)));
                }
            }
            Some(_) => {}
            None if q.is_empty() => {}
            None => {
                for (l, names) in &self.links {
                    if let Some(m) = q.prepared.best(names, &mut cx.matcher) {
                        out.push(Candidate::normal(Arc::new(Self::item(l, "")), m));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            looks_like_url("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            looks_like_url("www.example.com/x").as_deref(),
            Some("https://www.example.com/x")
        );
        assert_eq!(
            looks_like_url("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(looks_like_url("config.json"), None);
        assert_eq!(looks_like_url("notes.txt"), None);
        assert_eq!(looks_like_url("1.5"), None);
        assert_eq!(looks_like_url("hello world.com"), None);
    }

    #[test]
    fn templates_are_url_encoded() {
        assert_eq!(
            fill_template("https://x/?q={query}", "rust gpui&co"),
            "https://x/?q=rust+gpui%26co"
        );
        assert_eq!(
            fill_template("https://x/?q={query}", "微信"),
            "https://x/?q=%E5%BE%AE%E4%BF%A1"
        );
    }
}
