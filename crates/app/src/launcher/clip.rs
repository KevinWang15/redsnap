//! The clipboard view of the search window (docs/clipboard.md): a list of
//! entries on the left, a preview on the right. Opened with the clipboard
//! shortcut or by typing `cb ` in the search field.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Global, HighlightStyle, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, StyledText, div, img, prelude::FluentBuilder, px, rgb,
    rgba,
};
use redsnap_core::clipboard::{ClipStore, Content, Entry, Found, History, Kind};
use redsnap_platform::clipboard::{Target, Watcher};

use super::{Launcher, now_ms};

/// The clipboard store and watcher, for the window and the actions.
pub struct Clipboard {
    pub store: ClipStore,
    pub watcher: Option<Arc<Watcher>>,
    pub show_skipped: bool,
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
}

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
        self.clip.selected = keep
            .and_then(|id| found.iter().position(|f| history.entries[f.index].id == id))
            .unwrap_or(0);
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
        self.clip.selected = to.clamp(0, self.clip.found.len() as isize - 1) as usize;
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
            .child(self.clip_preview())
            .into_any_element()
    }

    fn clip_row(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let found = &self.clip.found[i];
        let e = &self.clip.history.entries[found.index];
        let selected = i == self.clip.selected;
        let skipped = e.kind == Kind::Skipped;
        let (glyph, title, ranges) = match e.kind {
            Kind::Text => {
                let (l, r) = first_line(&e.text, &found.highlights);
                ("T", l, r)
            }
            Kind::Image => ("▣", e.text.clone(), Vec::new()),
            Kind::Files => {
                let n = e.text.lines().count();
                let first = e.text.lines().next().unwrap_or("");
                let name = std::path::Path::new(first).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                ("▤", if n > 1 { format!("{name} and {} more", n - 1) } else { name }, Vec::new())
            }
            Kind::Skipped => ("–", format!("Not kept: {}", e.text), Vec::new()),
        };
        let app = e.source_app.clone().map(|a| a.trim_end_matches(".exe").to_string()).unwrap_or_else(|| "Unknown app".into());
        let subtitle = format!("{app} · {}{}", ago(e.last_copied_at), if e.pinned { " · pinned" } else { "" });
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
                    .text_size(px(15.))
                    .text_color(white(0.9))
                    .child(glyph),
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
            .into_any_element()
    }

    fn clip_preview(&self) -> AnyElement {
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
            Kind::Text => {
                // Long texts: the first 4,000 characters; the entry itself is complete.
                let end = e.text.char_indices().nth(4000).map_or(e.text.len(), |(i, _)| i);
                let ranges: Vec<Range<usize>> = found.highlights.iter().filter(|r| r.end <= end).cloned().collect();
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(white(0.9))
                    .child(highlighted(e.text[..end].to_string(), ranges))
                    .into_any_element()
            }
        };
        let info = match e.kind {
            Kind::Image => e.image_size.map(|(w, h)| format!("{w} × {h} · {}", redsnap_core::clipboard::human_bytes(e.size_bytes))),
            Kind::Text => Some(format!("{} characters", e.text.chars().count())),
            Kind::Files => Some(format!("{} item{}", e.text.lines().count(), if e.text.lines().count() == 1 { "" } else { "s" })),
            Kind::Skipped => None,
        };
        pane.flex()
            .flex_col()
            .gap(px(8.))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .when_some(info, |d, info| d.child(div().text_size(px(12.)).text_color(white(0.45)).child(info)))
            .into_any_element()
    }
}
