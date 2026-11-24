//! The clipboard view of the search window (docs/clipboard.md): a list of
//! entries on the left, a preview on the right. Opened with the clipboard
//! shortcut or by typing `cb ` in the search field.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::components::{MenuEntry, SelectableTextState, selectable_text};
use gpui::icons::LucideIcon;
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, Focusable, Global, HighlightStyle,
    InteractiveElement, IntoElement, ObjectFit, ParentElement, Pixels, RenderImage,
    ScrollbarVisibility, SharedString, StatefulInteractiveElement, Styled, StyledImage, StyledText,
    Window, div, img, prelude::FluentBuilder, px, rgb, rgba,
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
pub struct ClipView {
    pub history: Arc<History>,
    pub found: Vec<Found>,
    pub selected: usize,
    pub top: usize,
    /// The selected image entry's picture, once loaded: (entry id, image).
    pub image: Option<(String, Arc<RenderImage>)>,
    /// Selectable, virtualized, scrollable text view for the right-hand
    /// preview pane (text entries, file lists, and skip notes).
    pub preview: Entity<SelectableTextState>,
    /// Entry id and kind currently loaded into `preview`.
    preview_id: Option<String>,
    preview_kind: Option<Kind>,
    preview_highlights: Vec<Range<usize>>,
    /// Brief "Copied" confirmation shown in the preview footer after copying
    /// text from the preview pane.
    copied_feedback: bool,
    copied_gen: u64,
    /// The Tab panel's selected action, while it's open.
    pub panel: Option<usize>,
    /// After a delete, keep the selection at this position (the next entry).
    keep_index: Option<usize>,
}

impl ClipView {
    pub fn new(cx: &mut Context<Launcher>) -> Self {
        let preview = cx.new(|cx| {
            SelectableTextState::new(cx)
                .line_height(ROW)
                .selection_color(rgba(0xE12D_4666))
                .scrollbar_visibility(ScrollbarVisibility::Always)
                .scrollbar_color(white(0.55))
        });
        Self {
            history: Arc::new(History::default()),
            found: Vec::new(),
            selected: 0,
            top: 0,
            image: None,
            preview,
            preview_id: None,
            preview_kind: None,
            preview_highlights: Vec::new(),
            copied_feedback: false,
            copied_gen: 0,
            panel: None,
            keep_index: None,
        }
    }

    /// Resets transient view state while keeping the `preview` entity (and its
    /// subscriptions) alive.
    pub fn reset(&mut self, cx: &mut Context<Launcher>) {
        self.history = Arc::new(History::default());
        self.found.clear();
        self.selected = 0;
        self.top = 0;
        self.image = None;
        self.preview_id = None;
        self.preview_kind = None;
        self.preview_highlights.clear();
        self.copied_feedback = false;
        self.copied_gen = self.copied_gen.wrapping_add(1);
        self.panel = None;
        self.keep_index = None;
        self.preview.update(cx, |s, cx| s.set_text("", cx));
    }
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
    vec![
        ClipAction::Paste,
        ClipAction::Copy,
        if e.pinned {
            ClipAction::Unpin
        } else {
            ClipAction::Pin
        },
        ClipAction::Delete,
    ]
}

/// The text preview's line height.
const ROW: Pixels = px(20.);

const LIMIT: usize = 300;

fn white(a: f32) -> gpui::Hsla {
    let mut c: gpui::Hsla = rgb(0xFFFFFF).into();
    c.a = a;
    c
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

fn match_highlight_style() -> HighlightStyle {
    HighlightStyle {
        color: Some(rgb(0xFFFFFF).into()),
        background_color: Some(rgba(0xE8344E99).into()),
        ..Default::default()
    }
}

fn highlighted(text: String, ranges: Vec<Range<usize>>) -> StyledText {
    let style = match_highlight_style();
    StyledText::new(text).with_highlights(ranges.into_iter().map(move |r| (r, style)))
}

fn preview_text_for(
    kind: Kind,
    text: &str,
    highlights: &[Range<usize>],
) -> Option<(String, Vec<Range<usize>>)> {
    match kind {
        Kind::Text | Kind::Files => Some((text.to_string(), highlights.to_vec())),
        Kind::Skipped => Some((
            format!(
                "This copy wasn't kept: {text}.\n\nRedSnap records every copy it sees, so nothing disappears silently."
            ),
            Vec::new(),
        )),
        Kind::Image => None,
    }
}

impl Launcher {
    /// Searches the clipboard history for the current query (after `cb `).
    pub(super) fn refresh_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(c) = cx.try_global::<Clipboard>() else {
            return;
        };
        let keep = self
            .clip
            .found
            .get(self.clip.selected)
            .map(|f| self.clip.history.entries[f.index].id.clone());
        let history = c.store.history();
        let found = history.search(&self.clip_query(), c.show_skipped, LIMIT);
        self.clip.selected = match self.clip.keep_index.take() {
            Some(i) => i.min(found.len().saturating_sub(1)),
            None => keep
                .and_then(|id| found.iter().position(|f| history.entries[f.index].id == id))
                .unwrap_or(0),
        };
        self.clip.history = history;
        self.clip.found = found;
        self.clip_scroll();
        self.load_preview(cx);
    }

    /// The query without the `cb ` that switched to this view.
    fn clip_query(&self) -> String {
        let q = self.query.trim_start();
        q.strip_prefix("cb ")
            .map(str::to_string)
            .unwrap_or_else(|| {
                if self.clip_sticky {
                    q.to_string()
                } else {
                    String::new()
                }
            })
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
        self.clip.selected = to;
        self.clip_scroll();
        self.load_preview(cx);
        cx.notify();
    }

    pub(super) fn clip_context_menu_open(&self, cx: &App) -> bool {
        self.in_clipboard() && self.clip.preview.read(cx).is_context_menu_open()
    }

    pub(super) fn clip_preview_focused(&self, window: &Window, cx: &App) -> bool {
        self.in_clipboard()
            && self
                .clip
                .preview
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
    }

    pub(super) fn clip_clear_preview_focus_state(&mut self, cx: &mut Context<Self>) {
        self.clip.preview.update(cx, |s, cx| {
            s.close_context_menu(cx);
            s.clear_selection(cx);
        });
    }

    /// Shows a brief "Copied" notice in the preview footer after text is
    /// copied from the preview pane.
    pub(super) fn clip_flash_copied(&mut self, cx: &mut Context<Self>) {
        self.clip.copied_feedback = true;
        self.clip.copied_gen = self.clip.copied_gen.wrapping_add(1);
        let seq = self.clip.copied_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;
            this.update(cx, |l, cx| {
                if l.clip.copied_gen == seq {
                    l.clip.copied_feedback = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.clip
            .found
            .get(self.clip.selected)
            .map(|f| &self.clip.history.entries[f.index])
    }

    /// Loads the selected entry's preview (text into [`SelectableTextState`],
    /// or image bytes off the UI thread).
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.clip.found.get(self.clip.selected).cloned() else {
            if self.clip.preview_id.is_some() {
                self.clip.preview_id = None;
                self.clip.preview_kind = None;
                self.clip.preview_highlights.clear();
                self.clip.preview.update(cx, |s, cx| s.set_text("", cx));
            }
            return;
        };
        let e = &self.clip.history.entries[found.index];
        if let Some((text, highlights)) = preview_text_for(e.kind, &e.text, &found.highlights) {
            let same_entry = self.clip.preview_id.as_deref() == Some(e.id.as_str())
                && self.clip.preview_kind == Some(e.kind);
            let style = match_highlight_style();
            if same_entry {
                if self.clip.preview_highlights != highlights {
                    self.clip.preview_highlights = highlights.clone();
                    let styled = highlights.into_iter().map(|r| (r, style)).collect();
                    self.clip
                        .preview
                        .update(cx, |s, cx| s.set_highlights(styled, cx));
                }
            } else {
                self.clip.preview_id = Some(e.id.clone());
                self.clip.preview_kind = Some(e.kind);
                self.clip.preview_highlights = highlights.clone();
                self.clip.copied_feedback = false;
                let styled: Vec<_> = highlights.into_iter().map(|r| (r, style)).collect();
                self.clip.preview.update(cx, |s, cx| {
                    s.set_text(text, cx);
                    if !styled.is_empty() {
                        s.set_highlights(styled, cx);
                    }
                });
            }
            return;
        }

        if self.clip.preview_id.is_some() {
            self.clip.preview_id = None;
            self.clip.preview_kind = None;
            self.clip.preview_highlights.clear();
            self.clip.preview.update(cx, |s, cx| s.set_text("", cx));
        }
        if e.kind != Kind::Image || self.clip.image.as_ref().is_some_and(|(id, _)| *id == e.id) {
            return;
        }
        let id = e.id.clone();
        let Some(store) = cx.try_global::<Clipboard>().map(|c| c.store.clone()) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let Some(Content::Image { png, .. }) = store.fetch(&id) else {
                return None;
            };
            let mut rgba = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .ok()?
                .into_rgba8();
            for p in rgba.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            Some((
                id,
                Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                    rgba
                )])),
            ))
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

    /// Shift+PageDown / Shift+PageUp: scrolls the preview by most of a page.
    pub(super) fn clip_page(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.clip.preview_id.is_some() {
            self.clip
                .preview
                .update(cx, |s, cx| s.scroll_page(down, cx));
            cx.notify();
        }
    }

    pub(super) fn clip_current(&self) -> usize {
        self.clip.panel.unwrap_or(self.clip.selected)
    }

    /// Moves within the Tab panel if it's open; `false` otherwise.
    pub(super) fn clip_panel_move(&mut self, to: isize, cx: &mut Context<Self>) -> bool {
        let Some(n) = self.selected_entry().map(|e| actions_for(e).len()) else {
            return false;
        };
        let Some(sel) = &mut self.clip.panel else {
            return false;
        };
        *sel = to.clamp(0, n as isize - 1) as usize;
        cx.notify();
        true
    }

    pub(super) fn clip_toggle_panel(&mut self, cx: &mut Context<Self>) {
        self.clip.panel = if self.clip.panel.is_some() || self.selected_entry().is_none() {
            None
        } else {
            Some(0)
        };
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
    fn after_store(
        &mut self,
        op: impl FnOnce(&ClipStore) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = cx.try_global::<Clipboard>().map(|c| c.store.clone()) else {
            return;
        };
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
        let Some(e) = self.selected_entry() else {
            return;
        };
        let (id, pinned) = (e.id.clone(), !e.pinned);
        self.after_store(move |s| s.pin(&id, pinned), cx);
    }

    /// Ctrl+Delete / Cmd+Backspace: deletes the selected entry; the selection
    /// moves to the next one.
    pub(super) fn clip_delete(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else {
            return;
        };
        let id = e.id.clone();
        self.clip.keep_index = Some(self.clip.selected);
        self.clip.preview_id = None;
        self.clip.preview_kind = None;
        self.clip.preview_highlights.clear();
        self.clip.preview.update(cx, |s, cx| s.set_text("", cx));
        self.after_store(move |s| s.delete(&id), cx);
    }

    /// Enter: puts the entry on the clipboard and pastes it into the app that
    /// was in front. `paste: false` (Ctrl/Cmd+C) only copies.
    pub(super) fn clip_use(&mut self, paste: bool, cx: &mut Context<Self>) {
        let Some(e) = self.selected_entry() else {
            return;
        };
        if e.kind == Kind::Skipped {
            return;
        }
        let id = e.id.clone();
        let Some(c) = cx.try_global::<Clipboard>() else {
            return;
        };
        let (store, watcher) = (c.store.clone(), c.watcher.clone());
        let target: Option<Target> = if paste { self.paste_target } else { None };
        self.hide(cx);
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let content = store
                    .fetch(&id)
                    .ok_or_else(|| anyhow::anyhow!("the entry is gone"))?;
                let watcher =
                    watcher.ok_or_else(|| anyhow::anyhow!("clipboard capture isn't running"))?;
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
        let list: Vec<AnyElement> = (self.clip.top
            ..(self.clip.top + rows).min(self.clip.found.len()))
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
                let labels = actions_for(e)
                    .iter()
                    .map(|a| a.label().to_string())
                    .collect();
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
                let name = std::path::Path::new(first)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                (
                    ic::FILES,
                    if n > 1 {
                        format!("{name} and {} more", n - 1)
                    } else {
                        name
                    },
                    Vec::new(),
                )
            }
            Kind::Skipped => (ic::BAN, format!("Not kept: {}", e.text), Vec::new()),
        };
        let app = e
            .source_app
            .clone()
            .map(|a| a.trim_end_matches(".exe").to_string())
            .unwrap_or_else(|| "Unknown app".into());
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
                    .child(
                        div()
                            .text_size(px(15.))
                            .truncate()
                            .child(highlighted(title, ranges)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .truncate()
                            .text_color(if selected { white(0.8) } else { white(0.5) })
                            .child(subtitle),
                    ),
            )
            .when(e.pinned, |d| {
                d.child(super::view::icon_svg(
                    crate::assets::icon::PIN,
                    15.,
                    rgb(0xFFC94D).into(),
                ))
            })
            .into_any_element()
    }

    /// Renders the selectable, scrollable text preview with a right-click
    /// context menu offering Copy, Select All, and Copy All.
    fn text_preview(&self, kind: Kind, cx: &mut Context<Self>) -> AnyElement {
        let launcher = cx.entity();
        let preview = self.clip.preview.clone();
        let (size, color) = match kind {
            Kind::Files => (px(13.), white(0.9)),
            Kind::Skipped => (px(14.), white(0.75)),
            Kind::Text | Kind::Image => (px(14.), white(0.9)),
        };
        selectable_text(&self.clip.preview)
            .size_full()
            .text_size(size)
            .line_height(ROW)
            .text_color(color)
            .context_menu_builder(move |_selected, mut entries, _window, cx| {
                let full_text = preview.read(cx).text().clone();
                let is_empty = full_text.is_empty();
                let launcher = launcher.clone();
                entries.push(MenuEntry::Separator);
                entries.push(
                    MenuEntry::item_with_icon(
                        "Copy All",
                        LucideIcon::ClipboardCopy,
                        move |_, _, cx| {
                            if !full_text.is_empty() {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    full_text.to_string(),
                                ));
                                launcher.update(cx, |this, cx| this.clip_flash_copied(cx));
                            }
                        },
                    )
                    .with_disabled(is_empty),
                );
                entries
            })
            .into_any_element()
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
        let Some(found) = self.clip.found.get(self.clip.selected) else {
            return pane.into_any_element();
        };
        let e = &self.clip.history.entries[found.index];
        let body: AnyElement = match e.kind {
            Kind::Image => match &self.clip.image {
                Some((id, image)) if *id == e.id => img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                _ => div()
                    .text_color(white(0.5))
                    .child("Loading…")
                    .into_any_element(),
            },
            Kind::Text | Kind::Files | Kind::Skipped => self.text_preview(e.kind, cx),
        };
        let selected_chars = if e.kind != Kind::Image {
            self.clip
                .preview
                .read(cx)
                .selected_text()
                .map_or(0, |s| s.chars().count())
        } else {
            0
        };
        let base_info = match e.kind {
            Kind::Image => e.image_size.map(|(w, h)| {
                format!(
                    "{w} × {h} · {}",
                    redsnap_core::clipboard::human_bytes(e.size_bytes)
                )
            }),
            Kind::Text => Some(format!("{} characters", e.text.chars().count())),
            Kind::Files => {
                let n = e.text.lines().count();
                Some(format!("{n} item{}", if n == 1 { "" } else { "s" }))
            }
            Kind::Skipped => None,
        };
        let info = if e.kind != Kind::Image && self.clip.copied_feedback {
            Some(match base_info {
                Some(b) => format!("Copied · {b}"),
                None => "Copied".into(),
            })
        } else if selected_chars > 0 {
            Some(match base_info {
                Some(b) => format!("{selected_chars} selected · {b}"),
                None => format!("{selected_chars} selected"),
            })
        } else {
            base_info
        };
        pane.flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(body),
            )
            .when_some(info, |d, info| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(if self.clip.copied_feedback {
                            white(0.75)
                        } else {
                            white(0.45)
                        })
                        .child(info),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_text_extracts_text_files_and_skip_notes() {
        let two = [0..5, 6..11];
        assert_eq!(
            preview_text_for(Kind::Text, "hello\nworld", &two),
            Some(("hello\nworld".into(), two.to_vec()))
        );
        let file_ranges = [1..4, 8..11];
        assert_eq!(
            preview_text_for(Kind::Files, "/tmp/a\n/tmp/b", &file_ranges),
            Some(("/tmp/a\n/tmp/b".into(), file_ranges.to_vec()))
        );
        let (msg, hl) = preview_text_for(Kind::Skipped, "excluded app", &two).unwrap();
        assert!(msg.contains("excluded app"));
        assert!(hl.is_empty());
        assert_eq!(preview_text_for(Kind::Image, "Image 10×10", &[]), None);
    }
}
