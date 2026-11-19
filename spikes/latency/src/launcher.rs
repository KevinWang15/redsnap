//! The search window, drawn to docs/design.md, and its show/hide sequence.

use std::time::Instant;

use gpui::{
    AnyElement, AppContext, AsyncApp, BoxShadow, Context, Entity, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, SharedString, Styled, WeakEntity, Window, WindowHandle, actions, div, img,
    linear_color_stop, linear_gradient, point, prelude::FluentBuilder, px, rgb, rgba,
};

use crate::catalog::{Catalog, Hit};
use crate::os::{self, Native};
use crate::perf;
use crate::query_field::{QueryChanged, QueryField};

actions!(launcher, [SelectNext, SelectPrev, Run, Dismiss, Quit]);

const CONTEXT: &str = "Launcher";
pub const WIDTH: f32 = 760.;
pub const HEIGHT: f32 = 474.;
const ROWS: usize = 5;
/// Results kept beyond the visible rows, so scrolling has something to show.
const LIMIT: usize = 50;

pub fn bind_keys(cx: &mut gpui::App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("ctrl-n", SelectNext, c),
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("ctrl-p", SelectPrev, c),
        KeyBinding::new("enter", Run, c),
        KeyBinding::new("escape", Dismiss, c),
        KeyBinding::new("secondary-q", Quit, c),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintMode {
    /// gpui paints on its next vsync tick.
    Stock,
    /// Paint right after input; show hidden from the compositor until painted.
    Now,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub paint: PaintMode,
    /// Bench runs: never take focus, never hide on blur.
    pub bench: bool,
    /// Keep the window cloaked (Windows): it is placed and painted normally,
    /// but DWM never shows it and it gets no mouse input. Off-screen windows
    /// can't be used instead; Windows doesn't send them WM_PAINT.
    pub cloaked: bool,
    /// The panel's corner radius: 20 by design, or the OS's own window radius
    /// where the OS rounds the window (Windows 11).
    pub radius: f32,
    /// Opacity of the panel's dark fill; lower shows more of the blurred
    /// desktop behind it.
    pub alpha: f32,
}

pub struct Launcher {
    field: Entity<QueryField>,
    catalog: Catalog,
    hits: Vec<Hit>,
    selected: usize,
    top: usize,
    pub native: Option<Native>,
    pub visible: bool,
    opts: Options,
    _subs: Vec<gpui::Subscription>,
}

impl Launcher {
    pub fn new(opts: Options, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| QueryField::new("Search apps and commands", cx));
        let mut subs = vec![cx.subscribe(&field, |this, _, _: &QueryChanged, cx| this.query_changed(cx))];
        if !opts.bench {
            subs.push(cx.observe_window_activation(window, |this, window, cx| {
                if this.visible && !window.is_window_active() {
                    this.hide(cx);
                }
            }));
        }
        let mut this = Self {
            field,
            catalog: Catalog::fixture(),
            hits: Vec::new(),
            selected: 0,
            top: 0,
            native: None,
            visible: false,
            opts,
            _subs: subs,
        };
        this.hits = this.catalog.search("", LIMIT);
        this
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.field.read(cx).query();
        log::debug!("query {query:?}");
        self.hits = self.catalog.search(&query, LIMIT);
        self.selected = 0;
        self.top = 0;
        perf::results_ready();
        cx.notify();
        self.paint_soon(cx);
    }

    /// In `Now` mode, paint as soon as this update returns instead of on the
    /// next vsync tick.
    fn paint_soon(&self, cx: &mut Context<Self>) {
        if self.opts.paint == PaintMode::Now
            && os::CAN_PAINT_NOW
            && let Some(n) = self.native
        {
            cx.spawn(async move |_, _| os::paint_now(n)).detach();
        }
    }

    /// Fresh state for a new show: empty query, the default list.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.clear(cx));
        self.hits = self.catalog.search("", LIMIT);
        self.selected = 0;
        self.top = 0;
        perf::results_ready();
        window.focus(&self.field.focus_handle(cx));
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        let native = self.native;
        cx.spawn(async move |_, cx| {
            if cfg!(target_os = "macos") {
                cx.update(|cx| cx.hide()).ok();
            } else if let Some(n) = native {
                os::hide(n);
            }
        })
        .detach();
    }

    fn select(&mut self, to: usize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        perf::input_started();
        self.selected = to.min(self.hits.len() - 1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + ROWS {
            self.top = self.selected + 1 - ROWS;
        }
        perf::results_ready();
        cx.notify();
        self.paint_soon(cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected + 1, cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected.saturating_sub(1), cx);
    }

    fn run(&mut self, _: &Run, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.hits.get(self.selected) {
            log::info!("run: {}", self.catalog.apps[hit.app].name);
        }
        self.hide(cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        self.hide(cx);
    }

    /// Ctrl/Cmd+Q: write the summary next to the perf file and exit.
    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        perf::write_summary();
        cx.quit();
    }
}

/// Shows the window: reset, place, paint (hidden from the compositor in `Now`
/// mode), reveal, focus. Runs as a task so no OS call happens inside an update.
pub async fn show(handle: WindowHandle<Launcher>, this: WeakEntity<Launcher>, started: Instant, cx: &mut AsyncApp) {
    perf::show_started(started);
    let prepared = handle.update(cx, |launcher, window, cx| {
        launcher.reset(window, cx);
        launcher.visible = true;
        (launcher.native, launcher.opts)
    });
    let Ok((native, opts)) = prepared else { return };
    let _ = this;

    if cfg!(target_os = "macos") {
        // App-level activation; gpui orders the window front and paints.
        cx.update(|cx| cx.activate(true)).ok();
        handle.update(cx, |_, window, _| window.activate_window()).ok();
        perf::record("show_calls", started.elapsed().as_secs_f64() * 1000.0);
        return;
    }
    let Some(n) = native else { return };
    os::place(n, os::placement_at_cursor(n, (WIDTH, HEIGHT)));
    if opts.cloaked {
        os::set_cloaked(n, true);
        os::show(n);
        if opts.paint == PaintMode::Now && os::CAN_PAINT_NOW {
            os::paint_now(n);
        }
    } else if opts.paint == PaintMode::Now && os::CAN_PAINT_NOW {
        os::set_cloaked(n, true);
        os::show(n);
        os::paint_now(n);
        os::set_cloaked(n, false);
    } else {
        os::show(n);
    }
    perf::record("show_calls", started.elapsed().as_secs_f64() * 1000.0);
    // No `window.activate_window()` on Windows: gpui's version replays its
    // creation bounds and injects a fake Alt keypress (SendInput) to get the
    // foreground. The process that received the hotkey may take it directly.
    if !opts.bench {
        os::take_focus(n);
        if !cfg!(windows) {
            handle.update(cx, |_, window, _| window.activate_window()).ok();
        }
    }
}

// ----- drawing ---------------------------------------------------------------

fn white(alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(0xFFFFFF).into();
    c.a = alpha;
    c
}

const RED: u32 = 0xE8344E;

fn with_alpha(rgb: u32, alpha: f32) -> gpui::Rgba {
    rgba((rgb << 8) | (alpha.clamp(0., 1.) * 255.).round() as u32)
}

impl Render for Launcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_query = !self.field.read(cx).is_empty();
        let count = self.hits.len();
        let rows: Vec<AnyElement> = (self.top..(self.top + ROWS).min(count))
            .map(|i| self.row(i, i == self.selected, i - self.top))
            .collect();

        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .flex()
            .flex_col()
            .p(px(16.))
            .rounded(px(self.opts.radius))
            .border_1()
            .border_color(white(0.10))
            .bg(linear_gradient(
                180.,
                linear_color_stop(with_alpha(0x10181E, self.opts.alpha), 0.),
                linear_color_stop(with_alpha(0x161222, (self.opts.alpha + 0.04).min(1.)), 1.),
            ))
            .text_color(white(1.0))
            // Search row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(56.))
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .rounded(px(12.))
                            .bg(rgba(0x080C128C))
                            .border_1()
                            .border_color(rgba(0xE12D4673))
                            .child(self.field.clone())
                            .when(has_query, |d| {
                                d.child(
                                    div()
                                        .size(px(26.))
                                        .rounded_full()
                                        .bg(white(0.10))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(15.))
                                        .text_color(white(0.75))
                                        .child("×"),
                                )
                            }),
                    )
                    .child(logo_button()),
            )
            // Results
            .child(
                div()
                    .mt(px(12.))
                    .h(px(ROWS as f32 * 66.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(rows),
            )
            // Footer
            .child(
                div()
                    .mt_auto()
                    .h(px(44.))
                    .border_t_1()
                    .border_color(white(0.07))
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .child(div().size(px(7.)).rounded_full().bg(rgb(RED)))
                    .child(div().ml(px(8.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(white(0.85)).child("RedSnap"))
                    .child(div().mx(px(8.)).size(px(3.)).rounded_full().bg(white(0.4)))
                    .child(div().text_color(white(0.5)).child(SharedString::from(format!(
                        "{count} result{}",
                        if count == 1 { "" } else { "s" }
                    ))))
                    .child(div().flex_1())
                    .child(key_hint("↑↓", "Navigate"))
                    .child(div().w(px(16.)))
                    .child(key_hint("↵", "Open")),
            )
    }
}

impl Launcher {
    fn row(&self, i: usize, selected: bool, slot: usize) -> AnyElement {
        let app = &self.catalog.apps[self.hits[i].app];
        let hint: SharedString = if selected {
            "↵".into()
        } else if cfg!(target_os = "macos") {
            format!("⌘{}", slot + 1).into()
        } else {
            format!("Ctrl {}", slot + 1).into()
        };
        div()
            .h(px(62.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(14.))
            .rounded(px(12.))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(selected, |d| {
                d.bg(linear_gradient(90., linear_color_stop(rgb(0xB3263A), 0.), linear_color_stop(rgb(0x8E1F33), 1.)))
                    .border_color(rgba(0xFF788273))
                    .shadow(vec![BoxShadow {
                        color: rgba(0xC81E3259).into(),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(18.),
                        spread_radius: px(0.),
                    }])
            })
            .child(img(self.catalog.icons[app.icon].clone()).size(px(38.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().text_size(px(19.)).font_weight(gpui::FontWeight::MEDIUM).child(app.name.clone()))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(if selected { white(0.80) } else { white(0.55) })
                            .child(app.subtitle.clone()),
                    ),
            )
            .child(div().text_size(px(15.)).text_color(if selected { white(0.9) } else { white(0.45) }).child(hint))
            .into_any_element()
    }
}

fn key_hint(key: &'static str, label: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .h(px(20.))
                .min_w(px(20.))
                .px(px(4.))
                .rounded(px(5.))
                .border_1()
                .border_color(white(0.18))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(white(0.75))
                .child(key),
        )
        .child(div().text_color(white(0.60)).child(label))
}

/// The RedSnap button: a red rounded square inside four corner brackets.
fn logo_button() -> impl IntoElement {
    let bracket = |top: bool, left: bool| {
        let d = div().absolute().size(px(8.)).border_color(white(0.85));
        let d = if top { d.top(px(11.)).border_t_2() } else { d.bottom(px(11.)).border_b_2() };
        if left { d.left(px(11.)).border_l_2() } else { d.right(px(11.)).border_r_2() }
    };
    div()
        .relative()
        .size(px(44.))
        .rounded(px(12.))
        .bg(white(0.06))
        .border_1()
        .border_color(white(0.08))
        .flex()
        .items_center()
        .justify_center()
        .child(bracket(true, true))
        .child(bracket(true, false))
        .child(bracket(false, true))
        .child(bracket(false, false))
        .child(div().size(px(12.)).rounded(px(3.)).bg(rgb(RED)))
}

impl Focusable for Launcher {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}
