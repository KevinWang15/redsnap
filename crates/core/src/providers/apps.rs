//! Installed applications. The platform layer scans the OS and hands over
//! [`AppEntry`] values; the catalog is immutable and swapped whole when a
//! rescan finishes, so searching never waits on a scan.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::item::{Action, IconRef, Item, ItemId, LaunchTarget, ProviderId};
use crate::matcher::{Match, MatchClass, Names};
use crate::query::Query;
use crate::search::{Candidate, Provider, SearchContext};

/// One app as the platform found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// The app's identity on this OS (AUMID, bundle id, desktop file id). Two
    /// entries with the same key are the same app.
    pub key: String,
    /// The display name, localized where the OS offers it.
    pub name: String,
    /// More names it can be found by: executable or bundle name, the
    /// unlocalized name, desktop-file keywords.
    pub aliases: Vec<String>,
    /// Shown under the name (its path, usually).
    pub subtitle: String,
    pub launch: LaunchTarget,
    /// For "show in folder", when the app is a file.
    pub location: Option<PathBuf>,
}

struct App {
    item: Arc<Item>,
    names: Names,
}

pub struct AppCatalog {
    apps: Vec<App>,
}

impl AppCatalog {
    /// Builds the searchable catalog: drops duplicate keys (the first wins) and
    /// hidden apps, and adds the user's aliases.
    pub fn build(
        entries: Vec<AppEntry>,
        aliases: &BTreeMap<String, Vec<String>>,
        hidden: &[String],
    ) -> Self {
        let hidden: HashSet<&str> = hidden.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        let apps = entries
            .into_iter()
            .filter_map(|entry| {
                let id = ItemId::new("app", &entry.key);
                if hidden.contains(id.as_str()) || !seen.insert(entry.key.clone()) {
                    return None;
                }
                let user = aliases
                    .get(id.as_str())
                    .into_iter()
                    .flatten()
                    .map(String::as_str);
                let names = Names::new(
                    &entry.name,
                    entry.aliases.iter().map(String::as_str).chain(user),
                );
                Some(App {
                    item: Arc::new(item(entry, id)),
                    names,
                })
            })
            .collect();
        AppCatalog { apps }
    }

    pub fn empty() -> Self {
        AppCatalog { apps: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }
}

pub struct AppsProvider {
    catalog: Arc<ArcSwap<AppCatalog>>,
}

impl AppsProvider {
    /// `catalog` is shared with whoever rescans; a new catalog is picked up
    /// on the next keystroke.
    pub fn new(catalog: Arc<ArcSwap<AppCatalog>>) -> Self {
        AppsProvider { catalog }
    }
}

fn item(entry: AppEntry, id: ItemId) -> Item {
    Item {
        icon: IconRef::App(id.clone()),
        id,
        provider: ProviderId::Apps,
        title: entry.name,
        subtitle: entry.subtitle,
        primary: Action::Launch(entry.launch),
        secondary: entry.location.map(Action::RevealPath),
        learnable: true,
    }
}

impl Provider for AppsProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Apps
    }

    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>) {
        if q.route.is_some() {
            return;
        }
        let catalog = self.catalog.load();
        if q.is_empty() {
            // The default list: every app, ranked by use.
            out.extend(catalog.apps.iter().map(|a| {
                Candidate::normal(
                    a.item.clone(),
                    Match {
                        class: MatchClass::Fuzzy,
                        score: 0,
                    },
                )
            }));
            return;
        }
        for app in &catalog.apps {
            if let Some(m) = q.prepared.best(&app.names, &mut cx.matcher) {
                out.push(Candidate::normal(app.item.clone(), m));
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn entry(key: &str, name: &str) -> AppEntry {
    AppEntry {
        key: key.into(),
        name: name.into(),
        aliases: Vec::new(),
        subtitle: format!("/Applications/{name}.app"),
        launch: LaunchTarget::Path(format!("/Applications/{name}.app").into()),
        location: Some(format!("/Applications/{name}.app").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_and_hidden_apps_are_dropped_and_aliases_added() {
        let mut aliases = BTreeMap::new();
        aliases.insert("app:code".to_string(), vec!["editor".to_string()]);
        let catalog = AppCatalog::build(
            vec![
                entry("code", "Visual Studio Code"),
                entry("code", "Code again"),
                entry("x", "Hidden"),
            ],
            &aliases,
            &["app:x".to_string()],
        );
        assert_eq!(catalog.len(), 1);
        let q = crate::query::Query::parse("editor", &Default::default());
        let m = q
            .prepared
            .best(&catalog.apps[0].names, &mut crate::matcher::new_matcher())
            .unwrap();
        assert_eq!(m.class, MatchClass::Exact);
    }
}
