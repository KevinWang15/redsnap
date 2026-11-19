//! The settings window's view: the painted title bar, the page list, the
//! page, and a floating notice. The pages are in `pages.rs`.
//!
//! Text fields commit when Enter is pressed or they lose focus. Each one is
//! kept with how to read its value from the config and how to write it back,
//! so a change made elsewhere updates every field that isn't being edited.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::components::*;
use gpui::icons::{Icon, IconSize, LucideIcon};
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, Context, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, WindowControlArea, div,
    prelude::FluentBuilder, px, rgb,
};
use redsnap_core::config::Config;
use serde_json::{Value, json};

use super::request;
use super::widgets::{display_shortcut, spec_from_keystroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    General,
    Search,
    Apps,
    Snippets,
    Clipboard,
    Advanced,
}

const PAGES: [(Page, &str, LucideIcon); 6] = [
    (Page::General, "General", LucideIcon::SlidersHorizontal),
    (Page::Search, "Search", LucideIcon::Search),
    (Page::Apps, "Apps", LucideIcon::AppWindow),
    (Page::Snippets, "Snippets", LucideIcon::SquarePen),
    (Page::Clipboard, "Clipboard", LucideIcon::ClipboardList),
    (Page::Advanced, "Advanced", LucideIcon::Wrench),
];

/// RedSnap's red.
pub const RED: u32 = 0xE8344E;

pub struct Snapshot {
    pub config: Config,
    pub revision: u64,
}

pub struct Notice {
    pub error: bool,
    pub text: SharedString,
}

/// Which shortcut the recorder is listening for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shortcut {
    Launcher,
    Clipboard,
}

type Get = Rc<dyn Fn(&Config) -> Option<String>>;
type Set = Rc<dyn Fn(&mut Config, &str) -> Result<(), String>>;

pub enum FieldState {
    Line(Entity<InputState>),
    Area(Entity<TextBoxState>),
}

/// A text field bound to a part of the config.
pub struct Field {
    pub state: FieldState,
    get: Get,
    _sub: Subscription,
}

impl Field {
    fn value(&self, cx: &App) -> String {
        match &self.state {
            FieldState::Line(s) => s.read(cx).value().to_string(),
            FieldState::Area(s) => s.read(cx).value().to_string(),
        }
    }

    fn set_value(&self, v: String, cx: &mut App) {
        match &self.state {
            FieldState::Line(s) => s.update(cx, |s, cx| s.set_value(v, cx)),
            FieldState::Area(s) => s.update(cx, |s, cx| s.set_value(v, cx)),
        }
    }

    fn focused(&self, window: &Window, cx: &App) -> bool {
        match &self.state {
            FieldState::Line(s) => s.read(cx).focus_handle(cx).is_focused(window),
            FieldState::Area(s) => s.read(cx).focus_handle(cx).is_focused(window),
        }
    }
}

pub struct SettingsView {
    pub snap: Option<Snapshot>,
    pub load_error: Option<String>,
    pub page: Page,
    pub notice: Option<Notice>,
    notice_gen: u64,
    pub info: Option<Value>,
    pub stats: Option<Value>,
    pub clip_stats: Option<Value>,
    pub fields: HashMap<String, Field>,
    /// Filters and draft rows: plain inputs not bound to the config.
    pub inputs: HashMap<String, (Entity<InputState>, Subscription)>,
    pub recording: Option<Shortcut>,
    pub recorder_error: Option<(Shortcut, String)>,
    pub recorder_focus: FocusHandle,
    /// A button waiting for its second click ("Click again to …").
    pub confirming: Option<&'static str>,
    window: AnyWindowHandle,
    _subs: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let recorder_focus = cx.focus_handle();
        let subs = vec![
            cx.on_blur(&recorder_focus, window, |this, _, cx| {
                this.stop_recording(cx)
            }),
            cx.on_release(|_, cx| {
                let _ = cx
                    .global::<crate::app::Events>()
                    .0
                    .unbounded_send(crate::app::Event::SettingsClosed);
            }),
        ];
        let mut this = SettingsView {
            snap: None,
            load_error: None,
            page: Page::General,
            notice: None,
            notice_gen: 0,
            info: None,
            stats: None,
            clip_stats: None,
            fields: HashMap::new(),
            inputs: HashMap::new(),
            recording: None,
            recorder_error: None,
            recorder_focus,
            confirming: None,
            window: window.window_handle(),
            _subs: subs,
        };
        this.call("config.get", json!({}), cx, |this, r, window, cx| match r {
            Ok(v) => this.set_snapshot(&v, window, cx),
            Err(e) => this.load_error = Some(e),
        });
        this.call("app.info", json!({}), cx, |this, r, _, _| {
            this.info = r.ok()
        });
        this.call("apps.stats", json!({}), cx, |this, r, _, _| {
            this.stats = r.ok()
        });
        this.refresh_clip_stats(cx);
        this
    }

    /// Sends a request; `then` runs with the answer.
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<Value, String>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let rx = request(cx, method, params);
        let window = self.window;
        cx.spawn(async move |this, cx| {
            let r = rx
                .await
                .unwrap_or_else(|_| Err("RedSnap didn't answer".into()));
            window
                .update(cx, |_, window, cx| {
                    this.update(cx, |this, cx| {
                        then(this, r, window, cx);
                        cx.notify();
                    })
                })
                .ok();
        })
        .detach();
    }

    pub fn refresh_clip_stats(&mut self, cx: &mut Context<Self>) {
        self.call("clipboard.stats", json!({}), cx, |this, r, _, _| {
            this.clip_stats = r.ok()
        });
    }

    fn set_snapshot(&mut self, v: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let config = match Config::from_json(&v["config"].to_string()) {
            Ok(c) => c,
            Err(e) => {
                self.load_error = Some(format!("{e:#}"));
                return;
            }
        };
        let revision = v["revision"].as_u64().unwrap_or(0);
        self.snap = Some(Snapshot { config, revision });
        self.sync_fields(window, cx);
    }

    /// Fields show the config, except the one being edited; fields for
    /// things that are gone are dropped.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        let mut gone = Vec::new();
        for (key, f) in &self.fields {
            match (f.get)(&snap.config) {
                None => gone.push(key.clone()),
                Some(want) if !f.focused(window, cx) && f.value(cx) != want => {
                    f.set_value(want, cx)
                }
                Some(_) => {}
            }
        }
        for key in gone {
            self.fields.remove(&key);
        }
    }

    /// An event from the app loop: a change made elsewhere.
    pub fn pushed(
        &mut self,
        event: &str,
        params: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            "config.changed" => self.set_snapshot(&params, window, cx),
            "apps.stats" => self.stats = Some(params),
            _ => {}
        }
        cx.notify();
    }

    pub fn notify(&mut self, error: bool, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(Notice {
            error,
            text: text.into(),
        });
        self.notice_gen += 1;
        if !error {
            // "Saved" and the like go away by themselves; errors stay until closed.
            let generation = self.notice_gen;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(2500))
                    .await;
                this.update(cx, |this, cx| {
                    if this.notice_gen == generation {
                        this.notice = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Applies `change` to the config and sends it. RedSnap answers with the
    /// new revision, or why not (then its current settings are shown).
    pub fn save(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Config)) {
        let Some(snap) = &self.snap else { return };
        let mut next = snap.config.clone();
        change(&mut next);
        if next == snap.config {
            return;
        }
        let params = json!({ "config": next, "revision": snap.revision });
        self.call(
            "config.save",
            params,
            cx,
            move |this, r, window, cx| match r {
                Ok(v) => {
                    let revision = v["revision"].as_u64().unwrap_or(0);
                    this.snap = Some(Snapshot {
                        config: next,
                        revision,
                    });
                    this.sync_fields(window, cx);
                    this.notify(false, "Saved", cx);
                }
                Err(e) => {
                    this.notify(true, e, cx);
                    this.call("config.get", json!({}), cx, |this, r, window, cx| {
                        if let Ok(v) = r {
                            this.set_snapshot(&v, window, cx);
                        }
                    });
                }
            },
        );
    }

    /// A one-line field bound to the config, made on first use.
    pub fn line(
        &mut self,
        key: &str,
        placeholder: &str,
        cx: &mut Context<Self>,
        get: impl Fn(&Config) -> Option<String> + 'static,
        set: impl Fn(&mut Config, &str) -> Result<(), String> + 'static,
    ) -> Entity<InputState> {
        if let Some(Field {
            state: FieldState::Line(s),
            ..
        }) = self.fields.get(key)
        {
            return s.clone();
        }
        let initial = self
            .snap
            .as_ref()
            .and_then(|s| get(&s.config))
            .unwrap_or_default();
        let placeholder = placeholder.to_string();
        let state = cx.new(|cx| {
            InputState::new(cx)
                .default_value(initial)
                .placeholder(placeholder)
        });
        let set: Set = Rc::new(set);
        let get: Get = Rc::new(get);
        let sub = {
            let (set, get) = (set.clone(), get.clone());
            cx.subscribe(&state, move |this, state, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Submit(_) | InputEvent::Blur) {
                    let value = state.read(cx).value().to_string();
                    this.commit(value, &get, &set, cx);
                }
            })
        };
        self.fields.insert(
            key.to_string(),
            Field {
                state: FieldState::Line(state.clone()),
                get,
                _sub: sub,
            },
        );
        state
    }

    /// A multi-line field bound to the config (Ctrl+Enter or leaving it saves).
    pub fn area(
        &mut self,
        key: &str,
        placeholder: &str,
        cx: &mut Context<Self>,
        get: impl Fn(&Config) -> Option<String> + 'static,
        set: impl Fn(&mut Config, &str) -> Result<(), String> + 'static,
    ) -> Entity<TextBoxState> {
        if let Some(Field {
            state: FieldState::Area(s),
            ..
        }) = self.fields.get(key)
        {
            return s.clone();
        }
        let initial = self
            .snap
            .as_ref()
            .and_then(|s| get(&s.config))
            .unwrap_or_default();
        let placeholder = placeholder.to_string();
        let state = cx.new(|cx| {
            TextBoxState::new(cx)
                .default_value(initial)
                .placeholder(placeholder)
                .rows(4)
        });
        let set: Set = Rc::new(set);
        let get: Get = Rc::new(get);
        let sub = {
            let (set, get) = (set.clone(), get.clone());
            cx.subscribe(&state, move |this, state, e: &TextBoxEvent, cx| {
                if matches!(e, TextBoxEvent::Submit(_) | TextBoxEvent::Blur) {
                    let value = state.read(cx).value().to_string();
                    this.commit(value, &get, &set, cx);
                }
            })
        };
        self.fields.insert(
            key.to_string(),
            Field {
                state: FieldState::Area(state.clone()),
                get,
                _sub: sub,
            },
        );
        state
    }

    fn commit(&mut self, value: String, get: &Get, set: &Set, cx: &mut Context<Self>) {
        let Some(snap) = &self.snap else { return };
        if get(&snap.config).as_deref() == Some(value.as_str()) {
            return;
        }
        let mut next = snap.config.clone();
        match set(&mut next, &value) {
            Ok(()) => self.save(cx, move |c| *c = next),
            Err(e) => self.notify(true, e, cx),
        }
    }

    /// A plain input (a filter, or a row being added), made on first use.
    /// `on` sees its changes and Enter.
    pub fn plain(
        &mut self,
        key: &str,
        placeholder: &str,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &Entity<InputState>, &InputEvent, &mut Context<Self>) + 'static,
    ) -> Entity<InputState> {
        if let Some((s, _)) = self.inputs.get(key) {
            return s.clone();
        }
        let placeholder = placeholder.to_string();
        let state = cx.new(|cx| InputState::new(cx).placeholder(placeholder));
        let sub = cx.subscribe(&state, move |this, state, e: &InputEvent, cx| {
            on(this, &state, e, cx)
        });
        self.inputs.insert(key.to_string(), (state.clone(), sub));
        state
    }

    pub fn plain_value(&self, key: &str, cx: &App) -> String {
        self.inputs
            .get(key)
            .map(|(s, _)| s.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    // ----- the shortcut recorder --------------------------------------------

    pub fn start_recording(
        &mut self,
        which: Shortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording == Some(which) {
            self.stop_recording(cx);
            return;
        }
        if self.recording.is_none() {
            // RedSnap's own shortcuts would take the keys being recorded.
            self.call("hotkey.suspend", json!({}), cx, |_, _, _, _| {});
        }
        self.recording = Some(which);
        self.recorder_error = None;
        window.focus(&self.recorder_focus, cx);
        cx.notify();
    }

    pub fn stop_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            self.call("hotkey.resume", json!({}), cx, |_, _, _, _| {});
            cx.notify();
        }
    }

    pub fn recorder_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(which) = self.recording else { return };
        cx.stop_propagation();
        let k = &e.keystroke;
        if k.key == "escape" && !k.modifiers.modified() {
            self.stop_recording(cx);
            return;
        }
        let Some(spec) = spec_from_keystroke(k) else {
            return;
        };
        self.stop_recording(cx);
        // Registered for real first, so a taken shortcut is refused here.
        self.call(
            "hotkey.test",
            json!({ "shortcut": spec }),
            cx,
            move |this, r, _, cx| match r {
                Ok(_) => this.save(cx, move |c| match which {
                    Shortcut::Launcher => c.hotkey = spec,
                    Shortcut::Clipboard => c.clipboard.hotkey = spec,
                }),
                Err(e) => this.recorder_error = Some((which, e)),
            },
        );
    }

    pub fn recorder(&self, which: Shortcut, value: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let recording = self.recording == Some(which);
        let error = self
            .recorder_error
            .as_ref()
            .filter(|(w, _)| *w == which)
            .map(|(_, e)| e.clone());
        let label: SharedString = if recording {
            "Press the new shortcut…".into()
        } else {
            display_shortcut(value).into()
        };
        let id = match which {
            Shortcut::Launcher => "rec-launcher",
            Shortcut::Clipboard => "rec-clipboard",
        };
        div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(4.))
            .child(
                div()
                    .id(id)
                    .when(recording, |d| d.track_focus(&self.recorder_focus))
                    .on_key_down(cx.listener(Self::recorder_key))
                    .h(px(32.))
                    .min_w(px(170.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.radius_md)
                    .border_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .map(|d| {
                        if recording {
                            d.border_color(rgb(RED))
                                .bg(theme.primary_soft)
                                .text_color(theme.primary_soft_foreground)
                        } else {
                            d.border_color(theme.input_border)
                                .bg(theme.input_bg)
                                .hover(|s| s.border_color(theme.input_border_hover))
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_recording(which, window, cx)
                    }))
                    .child(label),
            )
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .max_w(px(300.))
                        .text_size(px(12.))
                        .text_color(theme.destructive_hover)
                        .child(e),
                )
            })
            .into_any_element()
    }

    /// A button that acts on its second click, within a few seconds.
    pub fn confirm_button(
        &self,
        key: &'static str,
        label: &str,
        confirm: &str,
        cx: &mut Context<Self>,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let armed = self.confirming == Some(key);
        button(key)
            .label(if armed {
                confirm.to_string()
            } else {
                label.to_string()
            })
            .variant(if armed {
                ButtonVariant::Destructive
            } else {
                ButtonVariant::Secondary
            })
            .size(ComponentSize::Sm)
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.confirming == Some(key) {
                    this.confirming = None;
                    act(this, cx);
                } else {
                    this.confirming = Some(key);
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_secs(4)).await;
                        this.update(cx, |this, cx| {
                            if this.confirming == Some(key) {
                                this.confirming = None;
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
                cx.notify();
            }))
            .into_any_element()
    }

    // ----- the window ---------------------------------------------------------

    fn title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mac = cfg!(target_os = "macos");
        // On Windows the OS handles these areas itself (snapping, the system
        // menu); elsewhere the clicks do it.
        let os_handles = cfg!(windows);
        let control = |id: &'static str, icon: LucideIcon, area: WindowControlArea, close: bool| {
            div()
                .id(id)
                .h_full()
                .w(px(46.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .window_control_area(area)
                .map(|d| {
                    if close {
                        d.hover(|s| s.bg(rgb(0xE81123)).text_color(gpui::white()))
                    } else {
                        d.hover(|s| s.bg(theme.surface_hover))
                    }
                })
                .child(Icon::new(icon).size(IconSize::Sm))
        };
        let maximized = window.is_maximized();
        div()
            .id("title-bar")
            .flex_none()
            .h(px(38.))
            .w_full()
            .flex()
            .items_center()
            .bg(theme.sidebar_bg)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .id("title-drag")
                    .h_full()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(if mac { 80. } else { 14. }))
                    .window_control_area(WindowControlArea::Drag)
                    .when(!os_handles, |d| {
                        d.on_mouse_down(MouseButton::Left, |e, window, _| {
                            if e.click_count == 2 {
                                window.zoom_window();
                            } else {
                                window.start_window_move();
                            }
                        })
                    })
                    .when(!mac, |d| d.child(super::pages::mark(18.)))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.muted_foreground)
                            .child("RedSnap Settings"),
                    ),
            )
            .when(!mac, |d| {
                d.child(
                    div()
                        .h_full()
                        .flex()
                        .child(
                            control("min", LucideIcon::Minus, WindowControlArea::Min, false)
                                .when(!os_handles, |d| {
                                    d.on_click(|_, window, _| window.minimize_window())
                                }),
                        )
                        .child(
                            control(
                                "max",
                                if maximized {
                                    LucideIcon::Copy
                                } else {
                                    LucideIcon::Square
                                },
                                WindowControlArea::Max,
                                false,
                            )
                            .when(!os_handles, |d| {
                                d.on_click(|_, window, _| window.zoom_window())
                            }),
                        )
                        .child(
                            control("close", LucideIcon::X, WindowControlArea::Close, true)
                                .when(!os_handles, |d| {
                                    d.on_click(|_, window, _| window.remove_window())
                                }),
                        ),
                )
            })
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(4.))
            .child(super::pages::mark(26.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("RedSnap"),
            );
        let mut group = SidebarGroup::new(None::<SharedString>);
        for (i, (page, label, icon)) in PAGES.into_iter().enumerate() {
            group = group.child(
                SidebarItem::new(("page", i), label)
                    .icon(icon)
                    .active(self.page == page)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        this.confirming = None;
                        cx.notify();
                    })),
            );
        }
        Sidebar::new("pages")
            .width(px(210.))
            .header(header)
            .child(group)
    }

    fn notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let n = self.notice.as_ref()?;
        let theme = cx.theme();
        Some(
            div()
                .absolute()
                .bottom(px(20.))
                .right(px(20.))
                .max_w(px(440.))
                .flex()
                .items_start()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(theme.radius_lg)
                .border_1()
                .text_size(px(13.))
                .map(|d| {
                    if n.error {
                        d.border_color(rgb(RED).into_hsla_opacity(0.4))
                            .bg(rgb(0x2A1219))
                            .text_color(theme.foreground)
                    } else {
                        d.border_color(theme.border)
                            .bg(theme.surface_muted)
                            .text_color(theme.muted_foreground)
                    }
                })
                .child(div().flex_1().min_w_0().child(n.text.clone()))
                .when(n.error, |d| {
                    d.child(
                        div()
                            .id("notice-close")
                            .cursor_pointer()
                            .text_color(theme.muted_foreground)
                            .hover(|s| s.text_color(theme.foreground))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            }))
                            .child(Icon::new(LucideIcon::X).size(IconSize::Sm)),
                    )
                })
                .into_any_element(),
        )
    }
}

trait Opacity {
    fn into_hsla_opacity(self, a: f32) -> gpui::Hsla;
}

impl Opacity for gpui::Rgba {
    fn into_hsla_opacity(self, a: f32) -> gpui::Hsla {
        gpui::Hsla::from(self).opacity(a)
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let body: AnyElement = if let Some(e) = &self.load_error {
            centered(format!("Couldn't load the settings: {e}"), cx)
        } else if self.snap.is_none() {
            centered("Loading…".to_string(), cx)
        } else {
            let page = super::pages::render(self, window, cx);
            AppShell::new("page")
                // The window's own fill is under it already.
                .bg(gpui::transparent_black())
                .sidebar(self.sidebar(cx))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .px(px(32.))
                        .py(px(28.))
                        .child(
                            div()
                                .w_full()
                                .max_w(px(760.))
                                .flex()
                                .flex_col()
                                .gap(px(28.))
                                .child(page),
                        ),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.title_bar(window, cx))
            .child(div().flex_1().min_h_0().child(body))
            .children(self.notice(cx))
    }
}

fn centered(text: String, cx: &App) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(14.))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

/// The dark theme with RedSnap's red, for the settings window.
/// The dark theme with RedSnap's red, for the settings window. Where the OS
/// blurs what's behind the window (`window::LOOK`), the surfaces are
/// translucent so the blur shows, like the search window's matte glass; text
/// fields, menus and tooltips stay nearly opaque so they read well.
pub fn theme() -> Theme {
    let mut t = Theme::dark().with_accent(rgb(RED).into());
    if redsnap_platform::window::LOOK.blur {
        let tint = |c: u32, a: f32| gpui::Hsla::from(rgb(c)).opacity(a);
        // Layered: the window's fill, then the sidebar and cards over it.
        t.background = tint(0x0E1219, 0.55);
        t.sidebar_bg = tint(0x0A0D13, 0.35);
        t.surface = tint(0x161B25, 0.45);
        t.input_bg = tint(0x0B0F16, 0.9);
        t.secondary = tint(0x1E2430, 0.85);
        t.border = gpui::white().opacity(0.08);
        t.sidebar_border = gpui::white().opacity(0.06);
    }
    t
}
