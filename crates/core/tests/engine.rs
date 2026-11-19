//! The search engine end to end, over a small fixture catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use redsnap_core::config::Config;
use redsnap_core::item::{Action, LaunchTarget, SystemCommand};
use redsnap_core::providers::apps::{AppCatalog, AppEntry, AppsProvider};
use redsnap_core::providers::{
    CalculatorProvider, CommandsProvider, OpenProvider, QuickLinksProvider, WebSearchProvider,
};
use redsnap_core::query::{Keywords, Target};
use redsnap_core::search::{Engine, Hit};
use redsnap_core::usage::{Usage, UsageStore};

const NOW: i64 = 1_700_000_000_000;

fn app(key: &str, name: &str) -> AppEntry {
    AppEntry {
        key: key.into(),
        name: name.into(),
        aliases: Vec::new(),
        subtitle: format!("/Applications/{name}.app"),
        launch: LaunchTarget::Path(format!("/Applications/{name}.app").into()),
        location: None,
    }
}

fn engine() -> Engine {
    let apps = vec![
        app("safari", "Safari"),
        app("settings", "System Settings"),
        app("code", "Visual Studio Code"),
        app("chrome", "Google Chrome"),
        app("calendar", "Calendar"),
        app("color", "Color Desk"),
        app("wechat", "微信"),
        app("music", "网易云音乐"),
        app("terminal", "Terminal"),
    ];
    let catalog = Arc::new(ArcSwap::from_pointee(AppCatalog::build(apps, &BTreeMap::new(), &[])));
    let config = Config::default();
    let keywords = Keywords::new(
        config.web_searches.iter().map(|w| (w.keyword.clone(), Target::WebSearch(w.id.clone()))),
    );
    Engine::new(
        vec![
            Box::new(CalculatorProvider),
            Box::new(OpenProvider { home: Some("/home/me".into()) }),
            Box::new(AppsProvider::new(catalog)),
            Box::new(CommandsProvider::new(&[SystemCommand::Lock, SystemCommand::Restart])),
            Box::new(QuickLinksProvider::new(Vec::new())),
            Box::new(WebSearchProvider::new(config.web_searches)),
        ],
        keywords,
    )
}

fn titles(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.item.title.as_str()).collect()
}

fn search(e: &mut Engine, q: &str, usage: &Arc<Usage>) -> Vec<Hit> {
    e.search(q, usage, NOW, 8).1
}

#[test]
fn ordinary_searches() {
    let mut e = engine();
    let u = Arc::new(Usage::default());
    assert_eq!(titles(&search(&mut e, "saf", &u))[0], "Safari");
    assert_eq!(titles(&search(&mut e, "vsc", &u))[0], "Visual Studio Code");
    assert_eq!(titles(&search(&mut e, "wx", &u))[0], "微信");
    assert_eq!(titles(&search(&mut e, "yinyue", &u))[0], "网易云音乐");
    assert_eq!(titles(&search(&mut e, "lock", &u))[0], "Lock Screen");
}

#[test]
fn literal_matches_come_before_fuzzy_ones() {
    let mut e = engine();
    let u = Arc::new(Usage::default());
    let hits = search(&mut e, "code", &u);
    assert_eq!(hits[0].item.title, "Visual Studio Code");
    // "Color Desk" contains c-o-d-e scattered; it may show, but only after.
    if let Some(i) = titles(&hits).iter().position(|t| *t == "Color Desk") {
        assert!(i > 0);
    }
}

#[test]
fn calculator_urls_and_paths_go_on_top() {
    let mut e = engine();
    let u = Arc::new(Usage::default());
    let hits = search(&mut e, "1/2", &u);
    assert_eq!(hits[0].item.title, "0.5");
    assert_eq!(hits[0].item.primary, Action::CopyText("0.5".into()));
    let hits = search(&mut e, "github.com", &u);
    assert_eq!(hits[0].item.primary, Action::OpenUrl("https://github.com".into()));
    let hits = search(&mut e, "~/dev", &u);
    assert_eq!(hits[0].item.primary, Action::OpenPath("/home/me/dev".into()));
}

#[test]
fn keyword_routes_to_a_web_search() {
    let mut e = engine();
    let u = Arc::new(Usage::default());
    let hits = search(&mut e, "g rust gpui", &u);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].item.primary, Action::OpenUrl("https://www.google.com/search?q=rust+gpui".into()));
}

#[test]
fn web_fallback_only_when_nothing_matches() {
    let mut e = engine();
    let u = Arc::new(Usage::default());
    let hits = search(&mut e, "zqxv kplm", &u);
    assert_eq!(titles(&hits), ["Search Google for “zqxv kplm”"]);
    assert!(!titles(&search(&mut e, "saf", &u)).iter().any(|t| t.starts_with("Search Google for")));
}

#[test]
fn picks_reorder_results_for_that_query() {
    let dir = tempfile::tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("usage.db")).unwrap();
    let mut e = engine();
    let before = search(&mut e, "c", &store.snapshot());
    let chrome = before.iter().find(|h| h.item.title == "Google Chrome").expect("chrome matches c").item.id.clone();
    assert_ne!(before[0].item.id, chrome);
    store.record_pick("c", &chrome, NOW);
    store.flush();
    let after = search(&mut e, "c", &store.snapshot());
    assert_eq!(after[0].item.title, "Google Chrome");
    // Learning stays within the match class: a literal "calendar" beats it.
    assert_eq!(search(&mut e, "calendar", &store.snapshot())[0].item.title, "Calendar");
}

#[test]
fn empty_query_lists_most_used_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = UsageStore::open(&dir.path().join("usage.db")).unwrap();
    let mut e = engine();
    let terminal = search(&mut e, "terminal", &store.snapshot())[0].item.id.clone();
    store.record_pick("terminal", &terminal, NOW);
    store.flush();
    assert_eq!(search(&mut e, "", &store.snapshot())[0].item.title, "Terminal");
}

#[test]
fn sn_lists_every_snippet_and_nothing_else() {
    use redsnap_core::config::Snippet;
    use redsnap_core::providers::SnippetsProvider;
    let snippets: Vec<Snippet> = (0..80)
        .map(|i| Snippet { id: format!("s{i}"), name: format!("Snippet {i}"), trigger: format!(";s{i}x"), text: "t".into(), auto_expand: true })
        .collect();
    let keywords = Keywords::new([("sn".to_string(), Target::Snippets)]);
    let apps = vec![app("snap", "Snap Camera")];
    let catalog = Arc::new(ArcSwap::from_pointee(AppCatalog::build(apps, &BTreeMap::new(), &[])));
    let mut e = Engine::new(vec![Box::new(AppsProvider::new(catalog)), Box::new(SnippetsProvider::new(&snippets))], keywords);
    let u = Arc::new(Usage::default());
    let all = e.search("sn ", &u, NOW, 200).1;
    assert_eq!(all.len(), 80);
    assert!(all.iter().all(|h| h.item.id.as_str().starts_with("snippet:")));
    let some = e.search("sn snippet 7", &u, NOW, 200).1;
    assert!(some.iter().any(|h| h.item.title == "Snippet 7"));
    // Without the space, "sn" is an ordinary query: the app is found too.
    assert!(e.search("sn", &u, NOW, 200).1.iter().any(|h| h.item.title == "Snap Camera"));
}
