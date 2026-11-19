//! The running app: settings, the usage store, the engine, the search
//! window, the tray, the shortcuts, clipboard capture, the settings app, and
//! commands from later launches. Everything that changes at run time lives in
//! one [`State`] owned by the event loop.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arc_swap::ArcSwap;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use gpui::{
    App, AppContext, Application, AsyncApp, BorrowAppContext, Bounds, RenderImage, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, point, px, size,
};
use redsnap_core::config::{self, Config};
use redsnap_core::files::{Files, FilesProvider};
use redsnap_core::item::{Action, ItemId};
use redsnap_core::providers::apps::{AppCatalog, AppEntry, AppsProvider};
use redsnap_core::providers::{
    CalculatorProvider, CommandsProvider, OpenProvider, QuickLinksProvider, SnippetsProvider, WebSearchProvider,
};
use redsnap_platform::snippets::{Expander, ExpanderConfig};
use redsnap_core::query::{Keywords, Target};
use redsnap_core::search::{Engine, Provider};
use redsnap_core::usage::UsageStore;
use redsnap_platform::instance::CliCommand;
use redsnap_platform::paths::Paths;
use redsnap_platform::settings_ipc::Reply;
use redsnap_platform::tray::{Tray, TrayCommand};
use redsnap_platform::{autostart, files, open, window as os};
use serde_json::{Value, json};

use crate::execute::Shell;
use crate::hotkeys::Hotkeys;
use crate::launcher::clip::Clipboard;
use crate::launcher::{self, Launcher};
use crate::settings::SettingsHost;

/// Everything that reaches the app from outside gpui, in one channel.
pub enum Event {
    Hotkey(Instant),
    /// The clipboard shortcut.
    ClipHotkey(Instant),
    Clip(redsnap_platform::clipboard::ClipEvent),
    Cli(CliCommand),
    Tray(TrayCommand),
    /// Scan installed apps again (startup, "Reload Apps", a stale catalog).
    Rescan,
    Scanned(Vec<AppEntry>),
    Icon(ItemId, Option<Arc<RenderImage>>),
    /// Open (or bring forward) the settings app.
    OpenSettings,
    /// A request from the settings app.
    Settings(String, Value, Reply),
    SettingsClosed,
    /// Type a snippet into the window that was in front.
    InsertSnippet(String, Option<redsnap_platform::clipboard::Target>),
    /// The file index or a folder listing answered.
    Files(redsnap_core::files::FileResults),
}

/// A rescan is due on show when the catalog is older than this.
const STALE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Lets code without the channel (actions) send app events.
pub struct Events(pub UnboundedSender<Event>);

impl gpui::Global for Events {}

pub struct Options {
    /// Start hidden (start at login uses this).
    pub background: bool,
}

/// The search engine for `config`, and the file search state it shares
/// with the launcher.
fn engine(config: &Config, catalog: Arc<ArcSwap<AppCatalog>>) -> (Engine, Arc<Files>) {
    let p = &config.providers;
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    if p.calculator {
        providers.push(Box::new(CalculatorProvider));
    }
    if p.open {
        providers.push(Box::new(OpenProvider));
    }
    // `f …` searches the index; typed paths (part of "open") list folders.
    let files = Arc::new(Files::new(p.files, p.open, files::BACKEND, files::SYNTAX, dirs::home_dir()));
    if p.files || p.open {
        providers.push(Box::new(FilesProvider(files.clone())));
    }
    if p.apps {
        providers.push(Box::new(AppsProvider::new(catalog)));
    }
    if p.commands {
        let system = if p.system_commands { open::system_commands() } else { &[] };
        providers.push(Box::new(CommandsProvider::new(system)));
    }
    if !config.snippets.items.is_empty() {
        providers.push(Box::new(SnippetsProvider::new(&config.snippets.items)));
    }
    if p.quick_links {
        providers.push(Box::new(QuickLinksProvider::new(config.quick_links.clone())));
    }
    if p.web_searches {
        providers.push(Box::new(WebSearchProvider::new(config.web_searches.clone())));
    }
    let keywords = Keywords::new(
        config
            .web_searches
            .iter()
            .filter(|_| p.web_searches)
            .map(|w| (w.keyword.clone(), Target::WebSearch(w.id.clone())))
            .chain(config.quick_links.iter().filter(|_| p.quick_links).map(|l| (l.keyword.clone(), Target::QuickLink(l.id.clone()))))
            // Built in, after the user's keywords (one of theirs named `sn` wins).
            .chain((!config.snippets.items.is_empty()).then(|| ("sn".to_string(), Target::Snippets)))
            .chain(p.files.then(|| ("f".to_string(), Target::Files))),
    );
    (Engine::new(providers, keywords), files)
}

/// Asks for a rescan of installed apps.
pub fn reload_apps(cx: &mut App) {
    let _ = cx.global::<Events>().0.unbounded_send(Event::Rescan);
}

/// Asks for the settings app.
pub fn open_settings(cx: &mut App) {
    let _ = cx.global::<Events>().0.unbounded_send(Event::OpenSettings);
}

/// What the event loop owns and changes.
struct State {
    paths: Paths,
    config: Config,
    /// Bumped on every saved change; a save based on an older one is refused.
    revision: u64,
    hotkeys: Hotkeys,
    catalog: Arc<ArcSwap<AppCatalog>>,
    usage: UsageStore,
    /// The last scan, so alias and hidden-app changes apply without rescanning.
    apps: Vec<AppEntry>,
    last_scan: Option<Instant>,
    /// When the last scan finished, for the settings app (ms since epoch).
    scan_ms: i64,
    scanning: bool,
    tray: Option<Tray>,
    /// Expands snippet triggers as they're typed (Windows).
    expander: Option<Expander>,
    settings: SettingsHost,
    events: UnboundedSender<Event>,
    handle: WindowHandle<Launcher>,
}

pub fn run(paths: Paths, opts: Options, events: UnboundedSender<Event>, mut rx: futures::channel::mpsc::UnboundedReceiver<Event>) -> Result<()> {
    let loaded = config::load(&paths.config)?;
    let config = loaded.config;
    let mut notice = loaded.problem.clone();
    if let Some(p) = &loaded.problem {
        log::warn!("{p}");
    }
    let usage = UsageStore::open(&paths.data.join("usage.db"))?;
    autostart::refresh();

    Application::new().with_assets(crate::assets::Assets).run(move |cx| {
        cx.set_global(Shell { paths: paths.clone() });
        cx.set_global(Events(events.clone()));
        let catalog = Arc::new(ArcSwap::from_pointee(AppCatalog::empty()));
        let icon_events = events.clone();
        cx.set_global(crate::icons::Icons::start(paths.cache.join("icons"), move |id, icon| {
            let _ = icon_events.unbounded_send(Event::Icon(id, icon));
        }));
        let _ = events.unbounded_send(Event::Rescan);
        crate::launcher::query_field::bind_keys(cx);
        launcher::bind_keys(cx);

        let rows = config.rows as usize;
        let lopts = launcher::Options { rows, blur: 28., alpha: 0.8 };
        let bounds = Bounds { origin: point(px(0.), px(0.)), size: size(px(launcher::WIDTH), px(launcher::height(rows))) };
        let window_engine = engine(&config, catalog.clone());
        let file_events = events.clone();
        let file_search = files::FileSearch::start(Box::new(move |r| {
            let _ = file_events.unbounded_send(Event::Files(r));
        }))
        .map_err(|e| log::warn!("file search: {e:#}"))
        .ok();
        let window_usage = usage.clone();
        let handle: WindowHandle<Launcher> = match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: false,
                show: false,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Transparent,
                app_id: Some("redsnap".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Launcher::new(window_engine, file_search, window_usage, lopts, window, cx)),
        ) {
            Ok(h) => h,
            Err(e) => {
                log::error!("can't create the search window: {e:#}");
                cx.quit();
                return;
            }
        };
        let native = handle
            .update(cx, |launcher, window, _| {
                launcher.native = os::native(window);
                launcher.native
            })
            .ok()
            .flatten();

        // The global shortcuts. If one is taken, RedSnap still runs (tray,
        // `redsnap toggle`) and says so when the window opens.
        let mut hotkeys = Hotkeys::new(events.clone());
        let clip_hotkey = config.clipboard.enabled.then_some(config.clipboard.hotkey.as_str());
        for problem in hotkeys.start(&config.hotkey, clip_hotkey) {
            log::warn!("{problem}");
            notice.get_or_insert(format!("{problem} Change it in Settings."));
        }

        // Clipboard history: the store, and the watcher that feeds it.
        if config.clipboard.enabled
            && let Err(e) = start_clipboard(&config, &paths, &events, cx)
        {
            notice.get_or_insert(format!("Clipboard history couldn't open its database: {e:#}"));
        }

        let expander = Expander::start(ExpanderConfig::from_config(&config.snippets))
            .map_err(|e| log::warn!("snippet expansion: {e:#}"))
            .ok()
            .flatten();

        let tray_events = events.clone();
        let tray = Tray::new(autostart::is_enabled(), Box::new(move |c| {
            let _ = tray_events.unbounded_send(Event::Tray(c));
        }))
        .map_err(|e| log::warn!("tray: {e:#}"))
        .ok();

        if notice.is_some() {
            handle.update(cx, |l, _, cx| l.set_notice(notice.clone(), cx)).ok();
        }
        // First start (or a problem to report) shows the window; start at
        // login stays in the background.
        if !opts.background || notice.is_some() {
            let _ = events.unbounded_send(Event::Cli(CliCommand::Show));
        }

        let mut state = State {
            paths: paths.clone(),
            config,
            revision: 1,
            hotkeys,
            catalog,
            usage,
            apps: Vec::new(),
            last_scan: None,
            scan_ms: 0,
            scanning: false,
            tray,
            expander,
            settings: SettingsHost::default(),
            events: events.clone(),
            handle,
        };
        cx.spawn(async move |cx| {
            // First, outside any update: style the window and make sure it's
            // hidden. Events (the first show included) are handled after.
            if let Some(n) = native {
                // No Windows rounding or border: the panel draws its own 20 px corners.
                os::setup(n, os::Clip::None, 20., false);
                os::hide(n);
            }
            while let Some(event) = rx.next().await {
                state.handle(event, cx).await;
            }
        })
        .detach();
    });
    Ok(())
}

fn start_clipboard(config: &Config, paths: &Paths, events: &UnboundedSender<Event>, cx: &mut App) -> Result<()> {
    let c = &config.clipboard;
    let limits = redsnap_core::clipboard::Limits { keep_days: c.keep_days, max_entries: c.max_entries as usize, ..Default::default() };
    let store = redsnap_core::clipboard::ClipStore::open(&paths.data.join("clipboard.db"), limits)?;
    let clip_events = events.clone();
    let read = redsnap_platform::clipboard::ReadLimits {
        max_text_bytes: limits.max_text_bytes,
        max_image_bytes: limits.max_image_bytes,
    };
    let watcher = redsnap_platform::clipboard::Watcher::start(
        read,
        Box::new(move |e| {
            let _ = clip_events.unbounded_send(Event::Clip(e));
        }),
    )
    .map_err(|e| log::warn!("clipboard capture: {e:#}"))
    .ok()
    .flatten();
    if watcher.is_none() {
        log::info!("clipboard capture isn't available on this OS yet");
    }
    cx.set_global(Clipboard {
        store,
        watcher: watcher.map(Arc::new),
        show_skipped: c.show_skipped,
        excluded_apps: c.excluded_apps.iter().map(|a| a.to_lowercase()).collect(),
    });
    Ok(())
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

impl State {
    async fn handle(&mut self, event: Event, cx: &mut AsyncApp) {
        let handle = self.handle;
        let visible = handle.update(cx, |l, _, _| l.visible).unwrap_or(false);
        match event {
            // The shortcuts toggle.
            Event::Hotkey(_) | Event::ClipHotkey(_) | Event::Cli(CliCommand::Toggle) if visible => {
                handle.update(cx, |l, _, cx| l.hide(cx)).ok();
            }
            Event::ClipHotkey(at) => launcher::show(handle, at, true, cx).await,
            Event::Hotkey(at) => {
                if self.last_scan.is_some_and(|t| t.elapsed() > STALE) && !self.scanning {
                    let _ = self.events.unbounded_send(Event::Rescan);
                }
                launcher::show(handle, at, false, cx).await
            }
            Event::Clip(e) => self.clipboard_event(e, visible, cx).await,
            Event::Rescan if self.scanning => {}
            Event::Rescan => {
                self.scanning = true;
                self.push_stats();
                let extra = self.config.extra_app_folders.clone();
                let tx = self.events.clone();
                cx.background_executor()
                    .spawn(async move {
                        let apps = redsnap_platform::apps::scan(&extra).unwrap_or_else(|e| {
                            log::warn!("scanning apps: {e:#}");
                            Vec::new()
                        });
                        let _ = tx.unbounded_send(Event::Scanned(apps));
                    })
                    .detach();
            }
            Event::Scanned(apps) => {
                self.scanning = false;
                self.scan_ms = crate::launcher::now_ms();
                self.last_scan = Some(Instant::now());
                self.apps = apps;
                self.rebuild_catalog(cx, true);
            }
            Event::Icon(id, icon) => {
                cx.update(|cx| cx.global::<crate::icons::Icons>().finished(id, icon)).ok();
                if visible {
                    handle.update(cx, |_, _, cx| cx.notify()).ok();
                }
            }
            Event::Cli(CliCommand::Toggle | CliCommand::Show) | Event::Tray(TrayCommand::Open) => {
                if !visible {
                    launcher::show(handle, Instant::now(), false, cx).await;
                }
            }
            Event::Cli(CliCommand::Settings) | Event::Tray(TrayCommand::Settings) | Event::OpenSettings => {
                if let Err(e) = self.settings.open(&self.paths, &self.events) {
                    log::warn!("opening settings: {e:#}");
                    // Without the settings app (a bare development build), the file itself.
                    let file = self.paths.config.join("config.json");
                    cx.update(|cx| crate::execute::run(Action::OpenPath(file), cx)).ok();
                }
            }
            Event::Settings(method, params, reply) => {
                let result = self.settings_request(&method, params, cx);
                if let Err(e) = &result {
                    log::info!("settings {method}: {e}");
                }
                reply(result);
            }
            Event::SettingsClosed => {
                log::info!("settings app closed");
                self.settings.closed();
                // A recorder left open when the window closed mustn't leave
                // the shortcuts off.
                for p in self.hotkeys.resume() {
                    log::warn!("{p}");
                }
            }
            Event::InsertSnippet(id, target) => {
                let Some(snippet) = self.config.snippets.items.iter().find(|s| s.id == id) else { return };
                let template = snippet.text.clone();
                let clipboard = cx.update(|cx| cx.read_from_clipboard().and_then(|i| i.text())).ok().flatten();
                let rendered = redsnap_core::snippets::render(
                    &template,
                    &redsnap_core::snippets::Context { now: chrono::Local::now(), clipboard },
                );
                std::thread::spawn(move || {
                    let result = match target {
                        Some(t) => redsnap_platform::snippets::insert(t, &rendered.text, rendered.cursor_back),
                        None => Err(anyhow::anyhow!("no app to insert into")),
                    };
                    if let Err(e) = result {
                        crate::execute::fail("Inserting the snippet", e);
                    }
                });
            }
            Event::Files(r) => {
                handle.update(cx, |l, _, cx| l.files_arrived(r, cx)).ok();
            }
            Event::Tray(TrayCommand::ToggleStartAtLogin) => {
                if let Err(e) = autostart::set(!autostart::is_enabled()) {
                    log::warn!("start at login: {e:#}");
                }
                if let Some(t) = &self.tray {
                    t.set_start_at_login(autostart::is_enabled());
                }
                self.push_config();
            }
            Event::Cli(CliCommand::Quit) | Event::Tray(TrayCommand::Quit) => {
                crate::perf::write_summary();
                self.settings.close();
                cx.update(|cx| cx.quit()).ok();
            }
        }
    }

    async fn clipboard_event(&mut self, e: redsnap_platform::clipboard::ClipEvent, visible: bool, cx: &mut AsyncApp) {
        use redsnap_core::clipboard::{Capture, Skip};
        use redsnap_platform::clipboard::ClipEventKind;
        let stored = cx
            .update(|cx| {
                let Some(c) = cx.try_global::<Clipboard>() else { return false };
                match e.kind {
                    // RedSnap pasted an entry; it already moved to the top.
                    ClipEventKind::OwnWrite(_) => false,
                    ClipEventKind::Captured(capture) => {
                        let app = e.source_app.as_deref().map(str::to_lowercase);
                        let excluded = app.as_ref().is_some_and(|a| {
                            c.excluded_apps.iter().any(|x| a == x || a.trim_end_matches(".exe") == x.trim_end_matches(".exe"))
                        });
                        let capture = if excluded { Capture::Skipped(Skip::ExcludedApp) } else { capture };
                        c.store.add(capture, e.source_app.clone(), e.at);
                        true
                    }
                }
            })
            .unwrap_or(false);
        if stored && visible {
            // The store commits on its own thread; look again shortly.
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            self.handle.update(cx, |l, _, cx| l.clipboard_changed(cx)).ok();
        }
    }

    /// A new catalog from the last scan and the current aliases / hidden apps.
    fn rebuild_catalog(&mut self, cx: &mut AsyncApp, warm_icons: bool) {
        let built = AppCatalog::build(self.apps.clone(), &self.config.app_aliases, &self.config.hidden_apps);
        log::info!("{} apps in the catalog", built.len());
        let warm: Vec<(ItemId, redsnap_core::item::LaunchTarget)> =
            if warm_icons { self.apps.iter().map(|a| (ItemId::new("app", &a.key), a.launch.clone())).collect() } else { Vec::new() };
        self.catalog.store(Arc::new(built));
        cx.update(|cx| cx.global::<crate::icons::Icons>().warm(warm)).ok();
        self.handle.update(cx, |l, _, cx| l.apps_changed(cx)).ok();
        self.push_stats();
    }

    fn config_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.config).unwrap_or(Value::Null);
        // The OS's start-at-login entry is the truth; the file mirrors it.
        v["startAtLogin"] = json!(autostart::is_enabled());
        json!({ "config": v, "revision": self.revision })
    }

    fn push_config(&self) {
        self.settings.push("config.changed", self.config_json());
    }

    fn stats(&self) -> Value {
        let apps: Vec<Value> = {
            let mut list: Vec<(String, String)> = self.apps.iter().map(|a| (a.name.clone(), format!("app:{}", a.key))).collect();
            list.sort_by_key(|(n, _)| n.to_lowercase());
            list.into_iter().map(|(name, id)| json!({ "name": name, "id": id })).collect()
        };
        json!({ "count": self.catalog.load().len(), "lastScanMs": self.scan_ms, "scanning": self.scanning, "apps": apps })
    }

    fn push_stats(&self) {
        self.settings.push("apps.stats", self.stats());
    }

    /// Requests from the settings app. Everything is validated here; the
    /// settings app only renders and edits.
    fn settings_request(&mut self, method: &str, params: Value, cx: &mut AsyncApp) -> Result<Value, String> {
        match method {
            "config.get" => Ok(self.config_json()),
            "settings.shown" => Ok(self.settings.shown()),
            "config.save" => self.save(params, cx),
            "app.info" => Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": std::env::consts::OS,
                "configDir": self.paths.config,
                "dataDir": self.paths.data,
            })),
            "hotkey.suspend" => {
                self.hotkeys.suspend();
                Ok(json!({}))
            }
            "hotkey.resume" => Ok(json!({ "problems": self.hotkeys.resume() })),
            "hotkey.test" => {
                let spec = params["shortcut"].as_str().ok_or("missing shortcut")?;
                self.hotkeys.test(spec).map(|_| json!({ "ok": true }))
            }
            "apps.stats" => Ok(self.stats()),
            "apps.reload" => {
                let _ = self.events.unbounded_send(Event::Rescan);
                Ok(json!({}))
            }
            "usage.reset" => self.usage.reset().map(|_| json!({})).map_err(err),
            "clipboard.stats" => Ok(cx
                .update(|cx| cx.try_global::<Clipboard>().map(|c| c.store.history().entries.len()))
                .ok()
                .flatten()
                .map_or(json!({ "available": false }), |n| json!({ "available": true, "entries": n }))),
            "clipboard.clear" => {
                cx.update(|cx| {
                    if let Some(c) = cx.try_global::<Clipboard>() {
                        c.store.clear();
                        c.store.flush();
                    }
                })
                .ok();
                Ok(json!({}))
            }
            "logs.open" => open::open_path(&self.paths.logs()).map(|_| json!({})).map_err(err),
            "data.open" => open::open_path(&self.paths.data).map(|_| json!({})).map_err(err),
            other => Err(format!("unknown request {other}")),
        }
    }

    fn restore_hotkeys(&mut self, old: &Config) {
        let clip = old.clipboard.enabled.then_some(old.clipboard.hotkey.as_str());
        let _ = self.hotkeys.apply(&old.hotkey, clip);
    }

    /// Validates and applies a changed config: shortcuts (registered before
    /// anything is written, rolled back if taken), start at login, providers
    /// and keywords, app aliases and hidden apps, clipboard options. Only
    /// then is `config.json` written and the change announced.
    fn save(&mut self, params: Value, cx: &mut AsyncApp) -> Result<Value, String> {
        let revision = params["revision"].as_u64().ok_or("missing revision")?;
        if revision != self.revision {
            return Err("stale: the settings changed meanwhile; showing the current ones".into());
        }
        let new = Config::from_json(&params["config"].to_string()).map_err(err)?;
        let old = self.config.clone();

        if new.hotkey != old.hotkey || new.clipboard.hotkey != old.clipboard.hotkey || new.clipboard.enabled != old.clipboard.enabled {
            let clip = new.clipboard.enabled.then_some(new.clipboard.hotkey.as_str());
            self.hotkeys.apply(&new.hotkey, clip)?;
        }
        if new.start_at_login != autostart::is_enabled() {
            if let Err(e) = autostart::set(new.start_at_login) {
                // Nothing of this change is kept: the shortcuts go back too.
                self.restore_hotkeys(&old);
                return Err(format!("start at login: {e:#}"));
            }
            if let Some(t) = &self.tray {
                t.set_start_at_login(autostart::is_enabled());
            }
        }
        if let Err(e) = config::save(&self.paths.config, &new) {
            self.restore_hotkeys(&old);
            return Err(format!("saving config.json: {e:#}"));
        }
        self.config = new;
        self.revision += 1;

        // Search: a new engine; aliases and hidden apps need a new catalog.
        let engine = engine(&self.config, self.catalog.clone());
        self.handle.update(cx, |l, _, cx| l.set_engine(engine, cx)).ok();
        if self.config.app_aliases != old.app_aliases || self.config.hidden_apps != old.hidden_apps {
            self.rebuild_catalog(cx, false);
        }
        if self.config.extra_app_folders != old.extra_app_folders {
            let _ = self.events.unbounded_send(Event::Rescan);
        }
        if self.config.snippets != old.snippets
            && let Some(x) = &self.expander
        {
            x.update(ExpanderConfig::from_config(&self.config.snippets));
        }
        // Clipboard options that apply at once (retention and turning
        // capture on or off apply at the next start).
        let c = self.config.clipboard.clone();
        cx.update(|cx| {
            if cx.has_global::<Clipboard>() {
                cx.update_global::<Clipboard, _>(|clip, _| {
                    clip.show_skipped = c.show_skipped;
                    clip.excluded_apps = c.excluded_apps.iter().map(|a| a.to_lowercase()).collect();
                });
            }
        })
        .ok();
        self.push_config();
        Ok(json!({ "revision": self.revision }))
    }
}
