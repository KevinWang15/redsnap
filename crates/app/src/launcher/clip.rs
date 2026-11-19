//! The clipboard view of the search window (docs/clipboard.md): a list of
//! entries on the left, a preview on the right. Opened with the clipboard
//! shortcut or by typing `cb ` in the search field.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Global, HighlightStyle, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage,
    ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage, StyledText,
    div, img, list, point, prelude::FluentBuilder, px, rgb, rgba,
};
use redsnap_core::clipboard::{ClipStore, Content, Entry, Found, History, Kind};
use redsnap_platform::clipboard::{Target, Watcher};

use super::{Launcher, now_ms};

/// The clipboard store and watcher, for the window and the actions.
pub struct Clipboard {
    pub store: ClipStore,
    pub watcher: Option<Arc<Watcher>>,
    pub show_skipped: bool,
    /// Lowercased app names whose copies are skipped.
    pub excluded_apps: Vec<String>,
}

impl Global for Clipboard {}

/// What the view shows right now.
#[derive(Default)]
pub struct ClipView {
    pub history: Arc<History>,
    pub found: Vec<Found>,
    pub selected: usize,
    pub top: usize,
    /// The selected image entry's picture, once loaded: (entry id, image).
    pub image: Option<(String, Arc<RenderImage>)>,
    /// The preview's scroll position (file lists, skip notes); back to the
    /// top for each entry.
    pub scroll: ScrollHandle,
    /// The text preview: the selected entry's lines, laid out only as they
    /// scroll into view, so texts of any length preview whole.
    pub text: Option<TextPreview>,
    /// The scrollbar thumb being dragged.
    drag: Option<Drag>,
    /// The Tab panel's selected action, while it's open.
    pub panel: Option<usize>,
    /// After a delete, keep the selection at this position (the next entry).
    keep_index: Option<usize>,
}

/// What the Tab panel offers for an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipAction {
    Paste,
    Copy,
    Pin,
    Unpin,
    Delete,
}

impl ClipAction {
    fn label(self) -> &'static str {
        match self {
            ClipAction::Paste => "Paste",
            ClipAction::Copy => "Copy",
            ClipAction::Pin => "Pin",
            ClipAction::Unpin => "Unpin",
            ClipAction::Delete => "Delete",
        }
    }
}

fn actions_for(e: &Entry) -> Vec<ClipAction> {
    if e.kind == Kind::Skipped {
        return vec![ClipAction::Delete];
    }
    vec![ClipAction::Paste, ClipAction::Copy, if e.pinned { ClipAction::Unpin } else { ClipAction::Pin }, ClipAction::Delete]
}

/// The text preview's line height.
const ROW: Pixels = px(20.);

/// Either kind of preview scroll area, behind one scrollbar.
#[derive(Clone)]
enum Scroller {
    List(ListState),
    Handle(ScrollHandle),
}

impl Scroller {
    /// The viewport (window coordinates), how far it can scroll, and how
    /// far it is scrolled (both positive, in pixels).
    ///
    /// The text list only knows the heights of rows it has laid out, so it is
    /// measured in rows instead: every row counts as one line tall
    /// (`ROW`), and position is the top row. Wrapped lines make that an
    /// estimate, but it is right at both ends and moves steadily.
    fn metrics(&self) -> (gpui::Bounds<Pixels>, Pixels, Pixels) {
        match self {
            Scroller::List(l) => {
                let view = l.viewport_bounds();
                let total = ROW * l.item_count() as f32;
                let top = l.logical_scroll_top();
                let y = ROW * top.item_ix as f32 + top.offset_in_item.min(ROW);
                let max = (total - view.size.height).max(px(0.));
                (view, max, y.min(max))
            }
            Scroller::Handle(h) => (h.bounds(), h.max_offset().height, -h.offset().y),
        }
    }

    fn scroll_to(&self, y: Pixels) {
        let (_, max, _) = self.metrics();
        let y = y.clamp(px(0.), max);
        match self {
            Scroller::List(l) => {
                let item_ix = (y / ROW).floor() as usize;
                l.scroll_to(gpui::ListOffset { item_ix, offset_in_item: y - ROW * item_ix as f32 });
            }
            Scroller::Handle(h) => h.set_offset(point(px(0.), -y)),
        }
    }
}

#[derive(Clone)]
struct Drag {
    scroller: Scroller,
    /// Mouse y and scroll position when the thumb was grabbed.
    grab_y: Pixels,
    start: Pixels,
    /// Scroll pixels per pixel of thumb movement.
    ratio: f32,
}

pub struct TextPreview {
    pub id: String,
    pub list: ListState,
    history: Arc<History>,
    index: usize,
    chunks: Arc<Vec<Range<usize>>>,
    highlights: Arc<Vec<Range<usize>>>,
}

/// Byte ranges of the lines of `text`, long lines cut into pieces so no
/// single row is expensive to lay out.
fn line_chunks(text: &str) -> Vec<Range<usize>> {
    const PIECE: usize = 2000;
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let mut from = 0;
        loop {
            let mut to = (from + PIECE).min(content.len());
            while !content.is_char_boundary(to) {
                to -= 1;
            }
            out.push(start + from..start + to);
            from = to;
            if from >= content.len() {
                break;
            }
        }
        start += line.len();
    }
    out
}

const LIMIT: usize = 300;


fn white(a: f32) -> gpui::Hsla {
    let mut c: gpui::Hsla = rgb(0xFFFFFF).into();
    c.a = a;
    c
}

/// The scrollbar thumb's height and top for a viewport `view` tall, content
/// scrollable by `max`, scrolled `y` down. `None` when nothing scrolls.
fn scroll_thumb(view: gpui::Pixels, max: gpui::Pixels, y: gpui::Pixels) -> Option<(gpui::Pixels, gpui::Pixels)> {
    (max > px(0.) && view > px(0.)).then(|| {
        let h = (view * (view / (view + max))).max(px(24.));
        (h, (view - h) * (y / max).clamp(0., 1.))
    })
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
fn ago(ms: i64) -> String {
    let s = ((now_ms() - ms) / 1000).max(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The first line of `text` (at most 200 characters) and the highlight
/// ranges that fall inside it.
fn first_line(text: &str, highlights: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let skip = text.len() - text.trim_start().len();
    let rest = &text[skip..];
    let mut end = rest.find('\n').unwrap_or(rest.len());
    if let Some((i, _)) = rest[..end].char_indices().nth(200) {
        end = i;
    }
    let line = rest[..end].trim_end().to_string();
    let ranges = highlights
        .iter()
        .filter(|r| r.start >= skip && r.end <= skip + line.len())
        .map(|r| r.start - skip..r.end - skip)
        .collect();
    (line, ranges)
}

fn highlighted(text: String, ranges: Vec<Range<usize>>) -> StyledText {
    let style = HighlightStyle {
        color: Some(rgb(0xFFFFFF).into()),
        background_color: Some(rgba(0xE8344E99).into()),
        ..Default::default()
    };
    StyledText::new(text).with_highlights(ranges.into_iter().map(move |r| (r, style)))
}

impl Launcher {
    /// Searches the clipboard history for the current query (after `cb `).
    pub(super) fn refresh_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(c) = cx.try_global::<Clipboard>() else { return };
        let keep = self.clip.found.get(self.clip.selected).map(|f| self.clip.history.entries[f.index].id.clone());
        let history = c.store.history();
        let found = history.search(&self.clip_query(), c.show_skipped, LIMIT);
        self.clip.selected = match self.clip.keep_index.take() {
            Some(i) => i.min(found.len().saturating_sub(1)),
            None => keep.and_then(|id| found.iter().position(|f| history.entries[f.index].id == id)).unwrap_or(0),
        };
        if self.clip.found.get(self.clip.selected).map(|f| &self.clip.history.entries[f.index].id)
            != found.get(self.clip.selected).map(|f| &history.entries[f.index].id)
        {
            self.clip.scroll.set_offset(point(px(0.), px(0.)));
        }
        self.clip.history = history;
        self.clip.found = found;
        self.clip_scroll();
        self.load_preview(cx);
    }

    /// The query without the `cb ` that switched to this view.
    fn clip_query(&self) -> String {
        let q = self.query.trim_start();
        q.strip_prefix("cb ").map(str::to_string).unwrap_or_else(|| if self.clip_sticky { q.to_string() } else { String::new() })
    }

    pub(super) fn clip_scroll(&mut self) {
        let rows = self.opts.rows;
        if self.clip.selected < self.clip.top {
            self.clip.top = self.clip.selected;
        } else if self.clip.selected >= self.clip.top + rows {
            self.clip.top = self.clip.selected + 1 - rows;
        }
    }

    pub(super) fn clip_move(&mut self, to: isize, cx: &mut Context<Self>) {
        if self.clip.found.is_empty() {
            return;
        }
        let to = to.clamp(0, self.clip.found.len() as isize - 1) as usize;
        if to != self.clip.selected {
            self.clip.scroll.set_offset(point(px(0.), px(0.)));
        }
        self.clip.selected = to;
        self.clip_scroll();
        self.load_preview(cx);
        cx.notify();
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.clip.found.get(self.clip.selected).map(|f| &self.clip.history.entries[f.index])
    }

    /// Loads the selected image entry's picture off the UI thread.
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else { return };
        if e.kind == Kind::Text && self.clip.text.as_ref().is_none_or(|t| t.id != e.id) {
            let found = &self.clip.found[self.clip.selected];
            let chunks = Arc::new(line_chunks(&e.text));
            self.clip.text = Some(TextPreview {
                id: e.id.clone(),
                list: ListState::new(chunks.len(), ListAlignment::Top, px(300.)),
                history: self.clip.history.clone(),
                index: found.index,
                chunks,
                highlights: Arc::new(found.highlights.clone()),
            });
            return;
        }
        if e.kind != Kind::Image || self.clip.image.as_ref().is_some_and(|(id, _)| *id == e.id) {
            return;
        }
        let id = e.id.clone();
        let Some(store) = cx.try_global::<Clipboard>().map(|c| c.store.clone()) else { return };
        let task = cx.background_executor().spawn(async move {
            let Some(Content::Image { png, .. }) = store.fetch(&id) else { return None };
            let mut rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?.into_rgba8();
            for p in rgba.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            Some((id, Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))))
        });
        cx.spawn(async move |this, cx| {
            if let Some(loaded) = task.await {
                this.update(cx, |l, cx| {
                    if l.selected_entry().is_some_and(|e| e.id == loaded.0) {
                        l.clip.image = Some(loaded);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Whether the preview's size is still unknown: scroll areas learn it
    /// while being laid out, so the scrollbar can only be drawn one frame
    /// later.
    pub(super) fn preview_unmeasured(&self) -> bool {
        let Some(e) = self.selected_entry() else { return false };
        match e.kind {
            Kind::Text => self.clip.text.as_ref().is_some_and(|t| t.list.viewport_bounds().size.height == px(0.)),
            Kind::Files | Kind::Skipped => self.clip.scroll.bounds().size.height == px(0.),
            Kind::Image => false,
        }
    }

    /// Shift+PageDown / Shift+PageUp: scrolls the preview by most of a page.
    pub(super) fn clip_page(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(t) = &self.clip.text
            && self.selected_entry().is_some_and(|e| e.id == t.id)
        {
            let page = t.list.viewport_bounds().size.height * 0.9;
            t.list.scroll_by(if down { page } else { -page });
            cx.notify();
            return;
        }
        let s = &self.clip.scroll;
        let page = s.bounds().size.height * 0.9;
        let max = s.max_offset().height;
        let y = s.offset().y + if down { -page } else { page };
        s.set_offset(point(px(0.), y.clamp(-max, px(0.))));
        cx.notify();
    }

    pub(super) fn clip_current(&self) -> usize {
        self.clip.panel.unwrap_or(self.clip.selected)
    }

    /// Moves within the Tab panel if it's open; `false` otherwise.
    pub(super) fn clip_panel_move(&mut self, to: isize, cx: &mut Context<Self>) -> bool {
        let Some(n) = self.selected_entry().map(|e| actions_for(e).len()) else { return false };
        let Some(sel) = &mut self.clip.panel else { return false };
        *sel = to.clamp(0, n as isize - 1) as usize;
        cx.notify();
        true
    }

    pub(super) fn clip_toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.clip.panel = if self.clip.panel.is_some() || self.selected_entry().is_none() { None } else { Some(0) };
        cx.notify();
    }

    /// Enter: the panel's action if it's open, otherwise paste.
    pub(super) fn clip_enter(&mut self, cx: &mut Context<Self>) {
        let action = match (self.clip.panel.take(), self.selected_entry()) {
            (Some(i), Some(e)) => actions_for(e).get(i).copied(),
            _ => Some(ClipAction::Paste),
        };
        match action {
            Some(ClipAction::Paste) => self.clip_use(true, cx),
            Some(ClipAction::Copy) => self.clip_use(false, cx),
            Some(ClipAction::Pin | ClipAction::Unpin) => self.clip_pin(cx),
            Some(ClipAction::Delete) => self.clip_delete(cx),
            None => cx.notify(),
        }
    }

    /// Runs `op` on the store, waits for it to commit (off the UI thread), then
    /// shows the new history.
    fn after_store(&mut self, op: impl FnOnce(&ClipStore) + Send + 'static, cx: &mut Context<Self>) {
        let Some(store) = cx.try_global::<Clipboard>().map(|c| c.store.clone()) else { return };
        let done = cx.background_executor().spawn(async move {
            op(&store);
            store.flush();
        });
        cx.spawn(async move |this, cx| {
            done.await;
            this.update(cx, |l, cx| {
                l.refresh_clipboard(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ctrl/Cmd+P: pins or unpins the selected entry. Pinned entries stay on
    /// top of the full list and are never removed by retention.
    pub(super) fn clip_pin(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else { return };
        let (id, pinned) = (e.id.clone(), !e.pinned);
        self.after_store(move |s| s.pin(&id, pinned), cx);
    }

    /// Ctrl+Delete / Cmd+Backspace: deletes the selected entry; the selection
    /// moves to the next one.
    pub(super) fn clip_delete(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else { return };
        let id = e.id.clone();
        self.clip.keep_index = Some(self.clip.selected);
        self.clip.text = None;
        self.after_store(move |s| s.delete(&id), cx);
    }

    /// Enter: puts the entry on the clipboard and pastes it into the app that
    /// was in front. `paste: false` (Ctrl/Cmd+C) only copies.
    pub(super) fn clip_use(&mut self, paste: bool, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else { return };
        if e.kind == Kind::Skipped {
            return;
        }
        let id = e.id.clone();
        let Some(c) = cx.try_global::<Clipboard>() else { return };
        let (store, watcher) = (c.store.clone(), c.watcher.clone());
        let target: Option<Target> = if paste { self.paste_target } else { None };
        self.hide(cx);
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let content = store.fetch(&id).ok_or_else(|| anyhow::anyhow!("the entry is gone"))?;
                let watcher = watcher.ok_or_else(|| anyhow::anyhow!("clipboard capture isn't running"))?;
                watcher.write(&content, &id)?;
                store.touch(&id, now_ms());
                if let Some(t) = target {
                    redsnap_platform::clipboard::paste_into(t)?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                crate::execute::fail("Pasting", e);
            }
        });
    }

    /// The left list and the right preview.
    pub(super) fn clip_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.opts.rows;
        let list: Vec<AnyElement> = (self.clip.top..(self.clip.top + rows).min(self.clip.found.len()))
            .map(|i| self.clip_row(i, cx))
            .collect();
        let empty = self.clip.found.is_empty();
        div()
            .relative()
            .mt(px(12.))
            .h(px(rows as f32 * 66.))
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .w(px(360.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(list)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .px(px(14.))
                                .py(px(20.))
                                .text_size(px(14.))
                                .text_color(white(0.5))
                                .child(if self.clip.history.entries.is_empty() {
                                    "Nothing copied yet. Copy something and it shows up here."
                                } else {
                                    "No entry contains all of these words."
                                }),
                        )
                    }),
            )
            .child(self.clip_preview(cx))
            .when_some(self.clip.panel.zip(self.selected_entry()), |d, (sel, e)| {
                let labels = actions_for(e).iter().map(|a| a.label().to_string()).collect();
                let title: SharedString = first_line(&e.text, &[]).0.into();
                d.child(super::view::panel_view(title, labels, sel))
            })
            .into_any_element()
    }

    fn clip_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let found = &self.clip.found[i];
        let e = &self.clip.history.entries[found.index];
        let selected = i == self.clip.selected;
        let skipped = e.kind == Kind::Skipped;
        use crate::assets::icon as ic;
        let (glyph, title, ranges) = match e.kind {
            Kind::Text => {
                let (l, r) = first_line(&e.text, &found.highlights);
                (ic::TYPE, l, r)
            }
            Kind::Image => (ic::IMAGE, e.text.clone(), Vec::new()),
            Kind::Files => {
                let n = e.text.lines().count();
                let first = e.text.lines().next().unwrap_or("");
                let name = std::path::Path::new(first).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                (ic::FILES, if n > 1 { format!("{name} and {} more", n - 1) } else { name }, Vec::new())
            }
            Kind::Skipped => (ic::BAN, format!("Not kept: {}", e.text), Vec::new()),
        };
        let app = e.source_app.clone().map(|a| a.trim_end_matches(".exe").to_string()).unwrap_or_else(|| "Unknown app".into());
        let subtitle = format!("{app} · {}", ago(e.last_copied_at));
        let slot = i - self.clip.top;
        div()
            .id(("clip", i))
            .h(px(62.))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(skipped, |d| d.opacity(0.55))
            .when(!selected, |d| d.hover(|s| s.bg(white(0.04))))
            .when(selected, |d| {
                d.bg(gpui::linear_gradient(
                    90.,
                    gpui::linear_color_stop(rgb(0xB3263A), 0.),
                    gpui::linear_color_stop(rgb(0x8E1F33), 1.),
                ))
                .border_color(rgba(0xFF788273))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.clip.selected = this.clip.top + slot;
                this.clip_use(true, cx);
            }))
            .child(
                div()
                    .size(px(32.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(rgb(0x3A4152))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(super::view::icon_svg(glyph, 17., white(0.9))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().text_size(px(15.)).truncate().child(highlighted(title, ranges)))
                    .child(
                        div()
                            .text_size(px(12.))
                            .truncate()
                            .text_color(if selected { white(0.8) } else { white(0.5) })
                            .child(subtitle),
                    ),
            )
            .when(e.pinned, |d| d.child(super::view::icon_svg(crate::assets::icon::PIN, 15., rgb(0xFFC94D).into())))
            .into_any_element()
    }

    /// The text preview: one row per line (or piece of a long line), laid out
    /// only when scrolled into view, with a thin scrollbar.
    fn text_list(&self, t: &TextPreview, cx: &mut Context<Self>) -> AnyElement {
        let (history, index, chunks, highlights) = (t.history.clone(), t.index, t.chunks.clone(), t.highlights.clone());
        let rows = list(t.list.clone(), move |i, _, _| {
            let text = &history.entries[index].text;
            let r = chunks[i].clone();
            let ranges: Vec<Range<usize>> = highlights
                .iter()
                .filter(|h| h.start >= r.start && h.end <= r.end)
                .map(|h| h.start - r.start..h.end - r.start)
                .collect();
            let line = &text[r];
            div()
                .w_full()
                .text_size(px(14.))
                .line_height(ROW)
                .min_h(ROW)
                .text_color(white(0.9))
                .child(highlighted(line.to_string(), ranges))
                .into_any_element()
        })
        .size_full();
        // Absolutely positioned, so the list gets a definite height and lays
        // out only the rows in view (an indefinite height measures them all).
        div()
            .relative()
            .size_full()
            .child(div().absolute().top_0().left_0().bottom_0().right(px(14.)).child(rows))
            .child(self.scrollbar(Scroller::List(t.list.clone()), cx))
            .into_any_element()
    }

    /// `content` in a vertical scroll area with a scrollbar.
    fn scrollable(&self, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.clip.scroll;
        div()
            .relative()
            .size_full()
            .child(div().id("clip-preview").size_full().overflow_y_scroll().track_scroll(s).pr(px(14.)).child(content))
            .child(self.scrollbar(Scroller::Handle(s.clone()), cx))
            .into_any_element()
    }

    /// The scrollbar on the right of a scroll area: the thumb widens and
    /// brightens on hover; drag it to scroll; click the track above or below
    /// it to move a page.
    fn scrollbar(&self, scroller: Scroller, cx: &mut Context<Self>) -> AnyElement {
        let (viewport, max, y) = scroller.metrics();
        let view = viewport.size.height;
        let Some((h, top)) = scroll_thumb(view, max, y) else { return div().into_any_element() };
        let dragging = self.clip.drag.is_some();
        let ratio = if view > h { max / (view - h) } else { 0. };
        div()
            .id("clip-scrollbar")
            .group("clip-scrollbar")
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(0.))
            .w(px(12.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let at = e.position.y - viewport.origin.y;
                    if at >= top && at <= top + h {
                        this.clip.drag = Some(Drag { scroller: scroller.clone(), grab_y: e.position.y, start: y, ratio });
                    } else {
                        // A page toward the click.
                        let page = view * 0.9;
                        scroller.scroll_to(if at < top { y - page } else { y + page });
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .right(px(2.))
                    .top(top)
                    .h(h)
                    .w(px(if dragging { 7. } else { 4. }))
                    .rounded(px(3.5))
                    .bg(white(if dragging { 0.55 } else { 0.25 }))
                    .group_hover("clip-scrollbar", |s| s.w(px(7.)).bg(white(0.45))),
            )
            .into_any_element()
    }

    /// While the thumb is held: follow the mouse.
    pub(super) fn clip_drag_move(&mut self, e: &MouseMoveEvent, _: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(d) = &self.clip.drag else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.clip.drag = None;
            cx.notify();
            return;
        }
        d.scroller.scroll_to(d.start + (e.position.y - d.grab_y) * d.ratio);
        cx.notify();
    }

    pub(super) fn clip_drag_end(&mut self, _: &MouseUpEvent, _: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.clip.drag.take().is_some() {
            cx.notify();
        }
    }

    fn clip_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        let pane = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .p(px(14.))
            .rounded(px(12.))
            .bg(rgba(0x0B0F16B3))
            .border_1()
            .border_color(white(0.07))
            .overflow_hidden();
        let Some(found) = self.clip.found.get(self.clip.selected) else { return pane.into_any_element() };
        let e = &self.clip.history.entries[found.index];
        let body: AnyElement = match e.kind {
            Kind::Image => match &self.clip.image {
                Some((id, image)) if *id == e.id => {
                    img(image.clone()).size_full().object_fit(ObjectFit::Contain).into_any_element()
                }
                _ => div().text_color(white(0.5)).child("Loading…").into_any_element(),
            },
            Kind::Files => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(13.))
                .children(e.text.lines().take(40).map(|l| div().truncate().child(l.to_string())))
                .into_any_element(),
            Kind::Skipped => div()
                .text_size(px(14.))
                .text_color(white(0.75))
                .child(SharedString::from(format!(
                    "This copy wasn't kept: {}.\n\nRedSnap records every copy it sees, so nothing disappears silently.",
                    e.text
                )))
                .into_any_element(),
            Kind::Text => match &self.clip.text {
                Some(t) if t.id == e.id => self.text_list(t, cx),
                _ => div().into_any_element(),
            },
        };
        // File lists and skip notes scroll here; text scrolls in its own list.
        let body = if matches!(e.kind, Kind::Files | Kind::Skipped) { self.scrollable(body, cx) } else { body };
        let info = match e.kind {
            Kind::Image => e.image_size.map(|(w, h)| format!("{w} × {h} · {}", redsnap_core::clipboard::human_bytes(e.size_bytes))),
            Kind::Text => Some(format!("{} characters", e.text.chars().count())),
            Kind::Files => Some(format!("{} item{}", e.text.lines().count(), if e.text.lines().count() == 1 { "" } else { "s" })),
            Kind::Skipped => None,
        };
        pane.flex()
            .flex_col()
            .gap(px(8.))
            .child(div().relative().flex_1().min_h_0().overflow_hidden().child(body))
            .when_some(info, |d, info| d.child(div().text_size(px(12.)).text_color(white(0.45)).child(info)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_long_lines_become_rows() {
        let t = "one\r\ntwo\n\nthree";
        let rows: Vec<&str> = line_chunks(t).into_iter().map(|r| &t[r]).collect();
        assert_eq!(rows, ["one", "two", "", "three"]);
        let long = "é".repeat(1500); // 3000 bytes
        let rows = line_chunks(&long);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| long.is_char_boundary(r.start) && long.is_char_boundary(r.end)));
        assert_eq!(rows.last().unwrap().end, long.len());
    }
}
