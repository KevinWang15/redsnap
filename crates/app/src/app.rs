//! The running app: settings, the usage store, the engine, the search
//! window, the tray, the hotkey, and commands from later launches.

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use arc_swap::ArcSwap;
use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{
    App, AppContext, Application, Bounds, RenderImage, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, point, px, size,
};
use redsnap_core::config::{self, Config};
use redsnap_core::item::{Action, ItemId};
use redsnap_core::providers::apps::{AppCatalog, AppEntry, AppsProvider};
use redsnap_core::providers::{CalculatorProvider, CommandsProvider, OpenProvider, QuickLinksProvider, WebSearchProvider};
use redsnap_core::query::{Keywords, Target};
use redsnap_core::search::{Engine, Provider};
use redsnap_core::usage::UsageStore;
use redsnap_platform::instance::CliCommand;
use redsnap_platform::paths::Paths;
use redsnap_platform::tray::{Tray, TrayCommand};
use redsnap_platform::{autostart, open, window as os};

use crate::execute::Shell;
use crate::launcher::{self, Launcher};

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

/// The app's shared catalog of installed apps; swapped whole by a rescan (M3).
struct Catalog(Arc<ArcSwap<AppCatalog>>);

impl gpui::Global for Catalog {}

fn engine(config: &Config, catalog: Arc<ArcSwap<AppCatalog>>) -> Engine {
    let p = &config.providers;
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    if p.calculator {
        providers.push(Box::new(CalculatorProvider));
    }
    if p.open {
        providers.push(Box::new(OpenProvider { home: dirs::home_dir() }));
    }
    if p.apps {
        providers.push(Box::new(AppsProvider::new(catalog)));
    }
    if p.commands {
        let system = if p.system_commands { open::system_commands() } else { &[] };
        providers.push(Box::new(CommandsProvider::new(system)));
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
            .chain(config.quick_links.iter().filter(|_| p.quick_links).map(|l| (l.keyword.clone(), Target::QuickLink(l.id.clone())))),
    );
    Engine::new(providers, keywords)
}

/// Asks for a rescan of installed apps.
pub fn reload_apps(cx: &mut App) {
    let _ = cx.global::<Events>().0.unbounded_send(Event::Rescan);
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

    Application::new().run(move |cx| {
        cx.set_global(Shell { paths: paths.clone() });
        let catalog = Arc::new(ArcSwap::from_pointee(AppCatalog::empty()));
        cx.set_global(Catalog(catalog.clone()));
        cx.set_global(Events(events.clone()));
        let icon_events = events.clone();
        cx.set_global(crate::icons::Icons::start(paths.cache.join("icons"), move |id, icon| {
            let _ = icon_events.unbounded_send(Event::Icon(id, icon));
        }));
        let _ = events.unbounded_send(Event::Rescan);
        let scan_config = config.clone();
        let mut last_scan: Option<Instant> = None;
        let mut scanning = false;
        crate::launcher::query_field::bind_keys(cx);
        launcher::bind_keys(cx);

        let rows = config.rows as usize;
        let lopts = launcher::Options { rows, blur: 28., alpha: 0.8 };
        let bounds = Bounds { origin: point(px(0.), px(0.)), size: size(px(launcher::WIDTH), px(launcher::height(rows))) };
        let engine = engine(&config, catalog);
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
            |window, cx| cx.new(|cx| Launcher::new(engine, usage, lopts, window, cx)),
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

        // The global shortcut. If it's taken, RedSnap still runs (tray,
        // `redsnap toggle`) and says so when the window opens.
        let manager = GlobalHotKeyManager::new().map_err(|e| log::error!("hotkeys unavailable: {e}")).ok();
        let mut register = |spec: &str, what: &str| -> Option<u32> {
            match (manager.as_ref(), spec.parse::<HotKey>()) {
                (Some(m), Ok(hotkey)) => match m.register(hotkey) {
                    Ok(()) => Some(hotkey.id()),
                    Err(e) => {
                        let msg = format!("The {what} shortcut {spec} is taken by another app ({e}). Change it in Settings.");
                        log::warn!("{msg}");
                        notice.get_or_insert(msg);
                        None
                    }
                },
                (_, Err(e)) => {
                    notice.get_or_insert(format!("The {what} shortcut {spec:?} isn't valid ({e}). Change it in Settings."));
                    None
                }
                (None, _) => None,
            }
        };
        let _main_id = register(&config.hotkey, "RedSnap");
        let clip_id = if config.clipboard.enabled { register(&config.clipboard.hotkey, "clipboard") } else { None };
        // Keep the manager (and its registration) for the life of the app.
        Box::leak(Box::new(manager));
        let hk = events.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                let event = if Some(e.id) == clip_id { Event::ClipHotkey(Instant::now()) } else { Event::Hotkey(Instant::now()) };
                let _ = hk.unbounded_send(event);
            }
        }));

        // Clipboard history: the store, and the watcher that feeds it.
        if config.clipboard.enabled {
            let c = &config.clipboard;
            let limits = redsnap_core::clipboard::Limits {
                keep_days: c.keep_days,
                max_entries: c.max_entries as usize,
                ..Default::default()
            };
            match redsnap_core::clipboard::ClipStore::open(&paths.data.join("clipboard.db"), limits) {
                Ok(store) => {
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
                    cx.set_global(crate::launcher::clip::Clipboard {
                        store,
                        watcher: watcher.map(Arc::new),
                        show_skipped: c.show_skipped,
                    });
                }
                Err(e) => {
                    notice.get_or_insert(format!("Clipboard history couldn't open its database: {e:#}"));
                }
            }
        }
        let excluded: Vec<String> = config.clipboard.excluded_apps.iter().map(|a| a.to_lowercase()).collect();

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

        let events_loop = events.clone();
        cx.spawn(async move |cx| {
            // First, outside any update: style the window and make sure it's
            // hidden. Events (the first show included) are handled after.
            if let Some(n) = native {
                // No Windows rounding or border: the panel draws its own 20 px corners.
                os::setup(n, os::Clip::None, 20., false);
                os::hide(n);
            }
            while let Some(event) = rx.next().await {
                log::debug!("event {}", match &event {
                    Event::Hotkey(_) => "hotkey".to_string(),
                    Event::ClipHotkey(_) => "clipboard hotkey".to_string(),
                    Event::Clip(c) => format!("clipboard change from {:?}", c.source_app),
                    Event::Cli(c) => format!("cli {c:?}"),
                    Event::Tray(t) => format!("tray {t:?}"),
                    Event::Rescan => "rescan".into(),
                    Event::Scanned(a) => format!("scanned {} apps", a.len()),
                    Event::Icon(id, _) => format!("icon {}", id.as_str()),
                });
                let visible = handle.update(cx, |l, _, _| l.visible).unwrap_or(false);
                match event {
                    // The shortcut toggles.
                    Event::Hotkey(_) | Event::ClipHotkey(_) | Event::Cli(CliCommand::Toggle) if visible => {
                        handle.update(cx, |l, _, cx| l.hide(cx)).ok();
                    }
                    Event::ClipHotkey(at) => launcher::show(handle, at, true, cx).await,
                    Event::Clip(e) => {
                        use redsnap_core::clipboard::{Capture, Skip};
                        use redsnap_platform::clipboard::ClipEventKind;
                        let stored = cx
                            .update(|cx| {
                                let Some(c) = cx.try_global::<crate::launcher::clip::Clipboard>() else { return false };
                                match e.kind {
                                    // RedSnap pasted an entry; it already moved to the top.
                                    ClipEventKind::OwnWrite(_) => false,
                                    ClipEventKind::Captured(capture) => {
                                        let app = e.source_app.as_deref().map(str::to_lowercase);
                                        let excluded_app = app.as_ref().is_some_and(|a| {
                                            excluded.iter().any(|x| a == x || a.trim_end_matches(".exe") == x.trim_end_matches(".exe"))
                                        });
                                        let capture = if excluded_app { Capture::Skipped(Skip::ExcludedApp) } else { capture };
                                        c.store.add(capture, e.source_app.clone(), e.at);
                                        true
                                    }
                                }
                            })
                            .unwrap_or(false);
                        if stored && visible {
                            // The store commits on its own thread; look again shortly.
                            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                            handle.update(cx, |l, _, cx| l.clipboard_changed(cx)).ok();
                        }
                    }
                    Event::Hotkey(at) => {
                        if last_scan.is_some_and(|t| t.elapsed() > STALE) && !scanning {
                            let _ = events_loop.unbounded_send(Event::Rescan);
                        }
                        launcher::show(handle, at, false, cx).await
                    }
                    Event::Rescan if scanning => {}
                    Event::Rescan => {
                        scanning = true;
                        let extra = scan_config.extra_app_folders.clone();
                        let tx = events_loop.clone();
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
                        scanning = false;
                        last_scan = Some(Instant::now());
                        let warm: Vec<(ItemId, redsnap_core::item::LaunchTarget)> =
                            apps.iter().map(|a| (ItemId::new("app", &a.key), a.launch.clone())).collect();
                        let built = AppCatalog::build(apps, &scan_config.app_aliases, &scan_config.hidden_apps);
                        log::info!("{} apps in the catalog", built.len());
                        cx.update(|cx| {
                            cx.global::<Catalog>().0.store(Arc::new(built));
                            cx.global::<crate::icons::Icons>().warm(warm);
                        })
                        .ok();
                        handle.update(cx, |l, _, cx| l.apps_changed(cx)).ok();
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
                    Event::Cli(CliCommand::Settings) | Event::Tray(TrayCommand::Settings) => {
                        cx.update(|cx| crate::execute::run(Action::RedSnap(redsnap_core::item::InternalCommand::OpenSettings), cx)).ok();
                    }
                    Event::Tray(TrayCommand::ToggleStartAtLogin) => {
                        let on = !autostart::is_enabled();
                        if let Err(e) = autostart::set(on) {
                            log::warn!("start at login: {e:#}");
                        }
                        if let Some(t) = &tray {
                            t.set_start_at_login(autostart::is_enabled());
                        }
                    }
                    Event::Cli(CliCommand::Quit) | Event::Tray(TrayCommand::Quit) => {
                        crate::perf::write_summary();
                        cx.update(|cx| cx.quit()).ok();
                    }
                }
            }
        })
        .detach();
    });
    Ok(())
}

